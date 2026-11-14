#!/usr/bin/env python3
"""Compare preserved JS binaries with Xcoder, offline, without changing installs.

Tool cells include compilation, sandbox startup, transport, and output handling.
The CLI fixture selects the preserved JS host through its existing exec adapter.
Only our descendants are sampled; no owner process is stopped or reconfigured.
"""
import argparse
from collections import defaultdict
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import platform
import queue
import resource
import statistics
import struct
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
NATIVE_API = 'using X :: #import "xcoder/runtime"\nusing J :: #import "std/json_document"\n'
MEMORY_KEYS = ('pss_kib', 'rss_kib', 'swappss_kib', 'pss_anon_kib', 'pss_file_kib')


def native(body):
    return NATIVE_API + 'main :: () -> s32 {\n' + body + '\nreturn 0\n}\n'


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def environment(directory):
    home = directory / 'home'
    home.mkdir()
    (directory / 'tmp').mkdir()
    env = {key: os.environ[key] for key in ('PATH', 'LANG', 'LC_ALL', 'XCODER_ZIRAN_TOOLCHAIN') if key in os.environ}
    env.update(HOME=str(home), CODEX_HOME=str(home / '.codex'), TMPDIR=str(directory / 'tmp'),
               XDG_CONFIG_HOME=str(home / '.config'), XDG_CACHE_HOME=str(home / '.cache'),
               XCODER_RUNTIME_DIR=str(directory / 'r'), RUST_LOG='error',
               NO_PROXY='127.0.0.1,localhost', HARMONY_SOURCE_SESSION='xcoder-offline-benchmark',
               HARMONY_MODEL_INPUT_DISABLE='1')
    return env


def tree_snapshot(root_pid):
    pending, seen, rows = [root_pid], set(), []
    while pending:
        pid = pending.pop()
        if pid in seen:
            continue
        seen.add(pid)
        proc = Path('/proc') / str(pid)
        try:
            for task in (proc / 'task').iterdir():
                try:
                    pending.extend(map(int, (task / 'children').read_text().split()))
                except (FileNotFoundError, ProcessLookupError):
                    pass
            values = {}
            for line in (proc / 'smaps_rollup').read_text().splitlines():
                fields = line.split()
                if fields[0] in ('Pss:', 'Rss:', 'SwapPss:', 'Pss_Anon:', 'Pss_File:'):
                    values[fields[0][:-1].lower() + '_kib'] = int(fields[1])
            executable = (proc / 'exe').stat()
            rows.append(dict(**values, pid=pid, executable=os.readlink(proc / 'exe'),
                             device=executable.st_dev, inode=executable.st_ino))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return {**{key: sum(row.get(key, 0) for row in rows) for key in MEMORY_KEYS},
            'processes': rows}


class Sampler:
    def __init__(self, pid):
        self.pid = pid
        self.phase = None
        self.lock = threading.Lock()
        self.peaks = defaultdict(dict)
        self.identities = set()
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def snapshot(self):
        value = tree_snapshot(self.pid)
        with self.lock:
            for row in value['processes']:
                self.identities.add((row['device'], row['inode']))
            if self.phase:
                peak = self.peaks[self.phase]
                for key in MEMORY_KEYS:
                    peak[key] = max(peak.get(key, 0), value[key])
        return value

    def run(self):
        while not self.stop.is_set():
            self.snapshot()
            self.stop.wait(0.02)

    def begin(self, name):
        with self.lock:
            self.phase = name
        self.snapshot()

    def close(self):
        self.stop.set()
        self.thread.join()


class Peer:
    def __init__(self, binary, directory):
        self.stderr = (directory / 'stderr.log').open('w+')
        self.started = time.perf_counter()
        self.process = subprocess.Popen([str(binary), '--listen', 'stdio://'],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.stderr, env=environment(directory), cwd=directory)
        self.sampler = Sampler(self.process.pid)
        self.queue = queue.Queue()
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        self.next_id = 0
        self.send(dict(type='connection/hello', supportedVersions=[1], requiredCapabilities=[],
                       optionalCapabilities=['bounded-output-v1', 'yield-observation',
                                             'session-cell-execution-resource-limits']))
        self.expect('connection/ready')
        self.operation('session/open', outputTruncation='server',
                       cellExecutionLimits=dict(maxHeapSizeBytes=268435456, maxYieldTimeMs=30000))
        self.expect('operation/response')
        self.startup_ms = (time.perf_counter() - self.started) * 1000

    def exact(self, count):
        value = bytearray()
        while len(value) < count:
            part = self.process.stdout.read(count - len(value))
            if not part:
                raise EOFError('host closed its output')
            value.extend(part)
        return value

    def read(self):
        try:
            while True:
                length, = struct.unpack('<I', self.exact(4))
                if length > 67108864:
                    raise ValueError('oversized host message')
                self.queue.put(json.loads(self.exact(length)))
        except BaseException as error:
            self.queue.put(error)

    def send(self, message):
        value = json.dumps(message).encode()
        self.process.stdin.write(struct.pack('<I', len(value)) + value)
        self.process.stdin.flush()

    def receive(self):
        while True:
            message = self.queue.get(timeout=65)
            if isinstance(message, BaseException):
                self.stderr.flush()
                self.stderr.seek(0)
                raise RuntimeError(self.stderr.read()) from message
            if message['type'] not in ('cell/closed', 'delegate/cancel'):
                return message

    def expect(self, kind):
        message = self.receive()
        if message['type'] != kind or message.get('result', {}).get('status', 'ok') != 'ok':
            raise AssertionError(message)
        return message

    def operation(self, method, **fields):
        self.next_id += 1
        self.send(dict(type='operation/request', id=self.next_id,
                       request=dict(method=method, sessionId='benchmark', **fields)))

    def execute(self, source, label, expected_calls=0, expected='ok', capture=False):
        self.sampler.begin(label)
        started = time.perf_counter()
        definitions = [dict(name='fixture', tool_name=dict(name='fixture', namespace=None),
                            description='offline echo', kind='function', input_schema=None, output_schema=None)]
        self.operation('session/execute', request=dict(tool_call_id='bench', enabled_tools=definitions,
                       source=source, yield_time_ms=30000, max_output_tokens=1000))
        self.expect('operation/response')
        calls = []
        while True:
            message = self.receive()
            if message['type'] == 'delegate/request':
                request = message['request']['invocation']
                value = request['input']
                if request['tool_name']['name'] != 'fixture':
                    raise AssertionError(request)
                calls.append(value)
                reply = {'payload': 'x' * 1048576} if value.get('large') else value
                self.send(dict(type='delegate/response', id=message['id'],
                               result=dict(status='ok', value=dict(type='tool/result', result=reply))))
            elif message['type'] == 'execute/initialResponse':
                result = message['result']['value']['Result']
                if result['error_text']:
                    raise AssertionError(result)
                content = ''.join(x['text'] for x in result['content_items'])
                if capture:
                    captured = json.loads(content)
                elif expected is None:
                    values = [json.loads(x['text']) for x in result['content_items']]
                    if (len(values) != expected_calls or
                        sorted(x['call'] for x in values) != list(range(expected_calls)) or
                        any(not x['ok'] or x['tool'] != 'fixture' or
                            x['value'] != ({} if expected_calls == 1 else {'n': x['call']}) for x in values)):
                        raise AssertionError(values)
                elif content != expected:
                    raise AssertionError(result)
                break
            else:
                raise AssertionError(message)
        elapsed = (time.perf_counter() - started) * 1000
        if len(calls) != expected_calls:
            raise AssertionError(calls)
        if expected_calls > 1 and sorted(x['n'] for x in calls) != list(range(expected_calls)):
            raise AssertionError(calls)
        settled = self.sampler.snapshot()
        sample = dict(ms=elapsed, settled_pss_kib=settled['pss_kib'], settled_pss_anon_kib=settled['pss_anon_kib'])
        if capture:
            sample['captured'] = captured
        return sample

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.sampler.close()
        self.reader.join(timeout=3)
        self.process.stdout.close()
        self.stderr.close()


def workloads(mode):
    zcall = 'r := Call("fixture", "{}"); defer Release(*r); if !r.ok { return 7 }; '
    inputs = ','.join(json.dumps(json.dumps(dict(n=n))) for n in range(16))
    zseq = 'inputs: [16]string = .[' + inputs + ']; for i: 0..15 { r := Call("fixture", inputs[i]); if !r.ok { return 7 }; Release(*r) }; Text("ok")'
    zpar = 'inputs: [16]string = .[' + inputs + ']; calls: [4]BatchCall; for i: 0..3 { calls[i].name="fixture"; calls[i].input=inputs[i] }; CallBatch(calls[:]); for i: 0..3 { if !calls[i].reply.ok { return 7 }; Release(*calls[i].reply) }; Text("ok")'
    large = '''r := Call("fixture", "{\\"large\\":true}"); defer Release(*r)
d: JsonDocument; defer JsonReset(*d); if !JsonRead(*d, Body(*r)) { return 7 }
value:=JsonMember(*d,d.root,"value"); payload:=JsonString(*d,JsonMember(*d,value,"payload"))
if payload.count != 1048576 { return 8 }; Text("ok")'''
    result = [
        ('output', native('Text("ok")'), 'text("ok")', 0, 'ok'),
        ('one_callback', native(zcall + 'Text("ok")'), 'await tools.fixture({}); text("ok")', 1, 'ok'),
        ('sequential_16', native(zseq), 'for(let n=0;n<16;n++) await tools.fixture({n}); text("ok")', 16, 'ok'),
        ('parallel_4', native(zpar), 'await Promise.all(Array.from({length:4},(_,n)=>tools.fixture({n}))); text("ok")', 4, 'ok'),
        ('json_1mib', native(large), 'const r=await tools.fixture({large:true}); if(r.payload.length!==1048576) throw Error("size"); text("ok")', 1, 'ok'),
    ]
    x = 1
    for _ in range(1000000):
        x = (x * 48271) % 2147483647
    result.append(('compute_1m', native('x: u64 = 1; for i: 0..999999 { x = (x * 48271) % 2147483647 }; if x != ' + str(x) + ' { return 7 }; Text("ok")'),
                   'let x=1; for(let i=0;i<1000000;i++) x=(x*48271)%2147483647; if(x!==' + str(x) + ') throw Error("result"); text("ok")', 0, 'ok'))
    if mode == 'plan':
        for index, (name, zi, js, calls, expected) in enumerate(result):
            if name not in ('one_callback', 'sequential_16', 'parallel_4'):
                continue
            inputs = [{}] if calls == 1 else [dict(n=n) for n in range(calls)]
            zi = json.dumps(dict(calls=[dict(tool='fixture', input=value) for value in inputs], parallel=name == 'parallel_4'))
            body = 'text(JSON.stringify({call:n,tool:"fixture",ok:true,value:await tools.fixture(input)}))'
            if calls == 1:
                js = 'const n=0,input={}; ' + body
            elif name == 'parallel_4':
                js = 'await Promise.all(Array.from({length:4},async(_,n)=>{const input={n};' + body + '}))'
            else:
                js = 'for(let n=0;n<16;n++){const input={n};' + body + '}'
            result[index] = name, zi, js, calls, None
    return result


def host_trial(engine, binary, directory, repeats, retention, mode):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    peer = Peer(binary, directory)
    result = dict(engine=engine, startup_ms=peer.startup_ms, opened_pss_kib=peer.sampler.snapshot()['pss_kib'], cases={})
    try:
        result['first_cell'] = peer.execute(native('Text("ok")') if engine == 'native' else 'text("ok")', 'first_cell')
        cases = workloads(mode)
        # Retain cold compile/JIT observations separately. A recurring first
        # custom-cell compile is a different phase, not a statistical anomaly.
        result['case_preparation'] = {
            name: peer.execute(zi if engine == 'native' else js, 'prepare_' + name, calls, expected)
            for name, zi, js, calls, expected in cases
        }
        # Rotate case order between repetitions, retaining all valid observations.
        for repetition in range(repeats):
            for offset in range(len(cases)):
                name, zi, js, calls, expected = cases[(offset + repetition) % len(cases)]
                sample = peer.execute(zi if engine == 'native' else js, name, calls, expected)
                result['cases'].setdefault(name, []).append(sample)
        result['retention'] = []
        for n in range(retention):
            sample = peer.execute(native('Text("ok")') if engine == 'native' else 'text("ok")', 'retention')
            if (n + 1) % 10 == 0 or n + 1 == retention:
                result['retention'].append(dict(cells=n + 1, pss_kib=sample['settled_pss_kib']))
        result['peaks'] = dict(peer.sampler.peaks)
        result['final_pss_kib'] = peer.sampler.snapshot()['pss_kib']
    finally:
        peer.close()
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    result['cpu_seconds_including_children'] = (after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)
    return result


def cli_trial(engine, cli, js_host, directory, cells, mode):
    env = environment(directory)
    config = Path(env['CODEX_HOME'])
    config.mkdir()
    requests, dispatches, latencies, post_call_pss, post_call_anon, errors = [], [], [], [], [], []
    state = {}
    ready = threading.Event()
    arguments = json.dumps(dict(cmd='printf xcoder-benchmark-marker', max_output_tokens=1000))
    source = (native('r := Call("exec_command", ' + json.dumps(arguments) + '); defer Release(*r); Text(Body(*r))')
              if engine == 'native' else 'text(await tools.exec_command(' + arguments + '))')
    if engine == 'native' and mode == 'plan':
        source = json.dumps(dict(calls=[dict(tool='exec_command', input=json.loads(arguments))]))
        env['XCODER_ZIRAN_TOOLCHAIN'] = str(directory / 'no-compiler')

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            try:
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                now = time.perf_counter()
                ready.wait()
                count = len(requests)
                requests.append(request)
                if count:
                    outputs = [x for x in request['input'] if x.get('type') == 'custom_tool_call_output' and x.get('call_id') == 'bench-' + str(count - 1)]
                    if len(outputs) != 1 or 'xcoder-benchmark-marker' not in json.dumps(outputs):
                        raise AssertionError('real shell tool did not return its marker: ' + json.dumps(outputs))
                    latencies.append((now - dispatches[-1]) * 1000)
                    snapshot = state['sampler'].snapshot()
                    if not any(row['pid'] == state['sampler'].pid for row in snapshot['processes']):
                        raise AssertionError('cannot sample the main CLI memory; refusing a partial process-tree result')
                    post_call_pss.append(snapshot['pss_kib'])
                    post_call_anon.append(snapshot['pss_anon_kib'])
                else:
                    state['startup_ms'] = (now - state['started']) * 1000
                if count > cells:
                    raise AssertionError('unexpected model request')
                item = (dict(type='custom_tool_call', call_id='bench-' + str(count), name='exec', input=source)
                        if count < cells else dict(type='message', role='assistant', id='done',
                        content=[dict(type='output_text', text='benchmark complete')]))
                events = [dict(type='response.created', response=dict(id='fixture-' + str(count))),
                          dict(type='response.output_item.done', item=item),
                          dict(type='response.completed', response=dict(id='fixture-' + str(count),
                               usage=dict(input_tokens=0, output_tokens=0, total_tokens=0)))]
                wire = ''.join('event: ' + x['type'] + '\ndata: ' + json.dumps(x) + '\n\n' for x in events).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(wire)))
                self.end_headers()
                dispatches.append(time.perf_counter())
                self.wfile.write(wire)
                self.wfile.flush()
            except BaseException as error:
                errors.append(repr(error))
                self.send_error(500)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    (config / 'config.toml').write_text(f'''model = "gpt-6.1-sol"
model_provider = "fixture"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
[analytics]
enabled = false
[features]
code_mode = true
code_mode_only = true
code_mode_host = true
enable_request_compression = false
[model_providers.fixture]
name = "Offline fixture"
base_url = "http://127.0.0.1:{server.server_address[1]}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
''')
    if engine == 'javascript':
        # The preserved Harmony adapter execs the stock host in this mode, so
        # there is no resident adapter process and no installed file changes.
        env.update(HARMONY_CODEX_EXEC='javascript', HARMONY_CODEX_STOCK_HOST=str(js_host))
    command = [str(cli), 'exec', '--ephemeral', '--skip-git-repo-check',
               '--sandbox', 'read-only', '--json', 'Run the offline benchmark.']
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    state['started'] = time.perf_counter()
    process = subprocess.Popen(command, cwd=directory, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    sampler = Sampler(process.pid)
    sampler.begin('cli')
    state['sampler'] = sampler
    ready.set()
    try:
        stdout, stderr = process.communicate(timeout=180)
        elapsed = (time.perf_counter() - state['started']) * 1000
        if process.returncode or errors or len(latencies) != cells or 'benchmark complete' not in stdout:
            raise AssertionError(dict(code=process.returncode, errors=errors, stdout=stdout, stderr=stderr))
        js_identity = (js_host.stat().st_dev, js_host.stat().st_ino)
        if (js_identity in sampler.identities) != (engine == 'javascript'):
            raise AssertionError('CLI did not use the expected native/JavaScript host')
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        sampler.close()
        server.shutdown()
        server.server_close()
        thread.join()
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return dict(engine=engine, total_ms=elapsed, startup_ms=state['startup_ms'], cell_ms=latencies,
                completion_to_exit_ms=elapsed - (dispatches[-1] - state['started']) * 1000,
                post_call_pss_kib=post_call_pss, post_call_pss_anon_kib=post_call_anon,
                sampled_peak=dict(sampler.peaks['cli']),
                javascript_host_observed=js_identity in sampler.identities,
                cpu_seconds_including_children=(after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime))


def stats(values):
    values = sorted(values)
    def percentile(fraction):
        position = (len(values) - 1) * fraction
        lo = int(position)
        hi = min(lo + 1, len(values) - 1)
        return values[lo] + (values[hi] - values[lo]) * (position - lo)
    return dict(n=len(values), median=statistics.median(values), minimum=values[0], maximum=values[-1],
                p90=percentile(.90), p95=percentile(.95))


def iqr_stats(values):
    ordered = sorted(values)
    def quantile(fraction):
        position = (len(ordered) - 1) * fraction
        lo = int(position)
        hi = min(lo + 1, len(ordered) - 1)
        return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)
    q1, q3 = quantile(.25), quantile(.75)
    width = q3 - q1
    excluded = [i for i, v in enumerate(values) if v < q1 - 1.5 * width or v > q3 + 1.5 * width]
    kept = [v for i, v in enumerate(values) if i not in excluded]
    return dict(raw=values, excluded=excluded, median=statistics.median(kept), rule='symmetric inclusive 1.5xIQR')


def json_diagnostic(args):
    """Decompose the actual V8/native host path; compilation is outside parse timers."""
    ziran = native('''r := Call("fixture", "{\\\"large\\\":true}"); defer Release(*r)
if !r.ok { return 7 }; text := Body(*r)
d: JsonDocument; defer JsonReset(*d)
print("{\\\"raw_ns_per_parse\\\":[")
for sample: 0..10 {
    started := MonotonicNanoseconds()
    for iteration: 0..511 {
        if !JsonRead(*d,text) { return 8 }
        value := JsonMember(*d,d.root,"value")
        payload := JsonString(*d,JsonMember(*d,value,"payload"))
        if payload.count != 1048576 || payload[0] != 120 || payload[payload.count - 1] != 120 { return 9 }
    }
    elapsed := cast(float64)(MonotonicNanoseconds() - started) / 512.0
    if sample > 0 { print(",") }; print("%",elapsed)
}
print("]}")
''').replace('main ::', 'using Timer :: #import "std/timer_linux"\nmain ::')
    javascript = '''const r=await tools.fixture({large:true});
const raw=JSON.stringify({ok:true,value:r}); const samples=[];
for(let sample=0;sample<11;sample++) {
  const start=Date.now();
  for(let i=0;i<512;i++) {
    const d=JSON.parse(raw),p=d.value.payload;
    if(p.length!==1048576||p.charCodeAt(0)!==120||p.charCodeAt(p.length-1)!==120) throw Error("payload");
  }
  samples.push((Date.now()-start)*1000000/512);
}
text(JSON.stringify({raw_ns_per_parse:samples}));'''
    raw_native = native('r := Call("fixture", "{\\\"large\\\":true}"); defer Release(*r); if !r.ok { return 7 }; Text("ok")')
    cases = {'native': ziran, 'javascript': javascript}
    report = dict(started_at=time.time(), host_hashes={e: sha(getattr(args, e + '_host')) for e in cases},
                  cpu_model=next(x.split(':',1)[1].strip() for x in Path('/proc/cpuinfo').read_text().splitlines() if x.startswith('model name')),
                  methodology='Actual Codex V8 vs sandboxed Zi2c; 11 batches of512parses, input construction/startup excluded from inner timer. JS Date.now resolution1ms; native monotonic clock. 11warm pipeline samples; cold invocation retained separately.',
                  blocks=[])
    with tempfile.TemporaryDirectory(dir=ROOT / 'backup', prefix='jp-') as temp:
        for block in range(3):
            for engine in (('javascript', 'native') if block % 2 == 0 else ('native', 'javascript')):
                directory = Path(temp) / (engine + str(block)); directory.mkdir()
                peer = Peer(getattr(args, engine + '_host'), directory)
                try:
                    row = dict(engine=engine, block=block)
                    row['inner'] = peer.execute(cases[engine], 'inner', 1, capture=True)
                    source = raw_native if engine == 'native' else 'await tools.fixture({large:true}); text("ok")'
                    row['raw_reply_cold'] = peer.execute(source, 'raw-cold', 1)
                    row['raw_reply'] = [peer.execute(source, 'raw', 1) for _ in range(11)]
                    source = workloads('source')[4][1 if engine == 'native' else 2]
                    row['json_cold'] = peer.execute(source, 'json-cold', 1)
                    row['json'] = [peer.execute(source, 'json', 1) for _ in range(11)]
                    row['peaks'] = dict(peer.sampler.peaks)
                    report['blocks'].append(row)
                    print(engine, block, row['inner']['captured'], flush=True)
                finally:
                    peer.close()
    report['finished_at'] = time.time()
    report['summary'] = {}
    for engine in cases:
        rows = [r for r in report['blocks'] if r['engine'] == engine]
        report['summary'][engine] = dict(
            parse_ns=iqr_stats([v for r in rows for v in r['inner']['captured']['raw_ns_per_parse']]),
            raw_reply_ms=iqr_stats([v['ms'] for r in rows for v in r['raw_reply']]),
            json_ms=iqr_stats([v['ms'] for r in rows for v in r['json']]))
    output = ROOT / 'backup/runtime-benchmark/json-diagnostic.json'
    output.write_text(json.dumps(report, indent=2) + '\n'); output.chmod(0o600)
    print(json.dumps(report['summary'], indent=2), flush=True)


def sandbox_regression_benchmark(args):
    """Measure the exact bwrap regression surface without LLM or network I/O."""
    native_host = args.native_host.resolve(strict=True)
    parent = ROOT / 'backup/runtime-benchmark'
    subprocess.run(['git', 'check-ignore', '-q', str(parent.relative_to(ROOT))], cwd=ROOT, check=True)
    parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    output = parent / ('sandbox-' + args.sandbox_regression_label + '.json')
    report = dict(
        started_at=time.time(),
        revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        workspace_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
        harness_sha256=sha(__file__),
        host_sha256=sha(native_host),
        kernel=platform.release(),
        label=args.sandbox_regression_label,
        methodology=(
            'Twenty isolated cache-miss Ziran cells, twenty cache-hit executions of one '
            'Ziran cell, and twenty precompiled JSON-plan calls through the same native host; '
            'offline fixture callback, wall-clock latency, no LLM inference.'
        ),
        samples={name: [] for name in ('cold_compile', 'warm_run', 'json_plan')},
    )

    def execute(peer, category, source, expected):
        started = time.perf_counter()
        try:
            sample = peer.execute(source, category, 1, expected)
            report['samples'][category].append(dict(ok=True, **sample))
        except (AssertionError, RuntimeError) as error:
            report['samples'][category].append(
                dict(ok=False, ms=(time.perf_counter() - started) * 1000, error=str(error))
            )

    with tempfile.TemporaryDirectory(dir=ROOT / 'backup', prefix='sr-') as temp:
        directory = Path(temp)
        peer = Peer(native_host, directory)
        try:
            for marker in range(20):
                source = native(
                    f'marker: u64 = {marker}; if marker != {marker} {{ return 9 }}; '
                    'r := Call("fixture", "{}"); defer Release(*r); '
                    'if !r.ok { return 7 }; Text("ok")'
                )
                execute(peer, 'cold_compile', source, 'ok')

            warm_source = native(
                'marker: u64 = 1000; if marker != 1000 { return 9 }; '
                'r := Call("fixture", "{}"); defer Release(*r); '
                'if !r.ok { return 7 }; Text("ok")'
            )
            # Populate the cache outside the measured warm sample set.
            execute(peer, 'warm_run', warm_source, 'ok')
            report['warm_preparation'] = report['samples']['warm_run'].pop()
            for _ in range(20):
                execute(peer, 'warm_run', warm_source, 'ok')

            plan = json.dumps(dict(calls=[dict(tool='fixture', input={})]))
            for _ in range(20):
                execute(peer, 'json_plan', plan, None)
        finally:
            peer.close()

    report['finished_at'] = time.time()
    report['summary'] = {}
    for category, samples in report['samples'].items():
        successful = [sample['ms'] for sample in samples if sample['ok']]
        failed = [sample['ms'] for sample in samples if not sample['ok']]
        report['summary'][category] = dict(
            successes=len(successful), failures=len(failed),
            success_ms=stats(successful) if successful else None,
            failure_ms=stats(failed) if failed else None,
        )
    output.write_text(json.dumps(report, indent=2) + '\n')
    output.chmod(0o600)
    print(json.dumps(dict(output=str(output), summary=report['summary']), indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--javascript-host', required=True, type=Path)
    parser.add_argument('--javascript-cli', type=Path)
    parser.add_argument('--native-host', type=Path, default=Path.home() / '.local/lib/xcoder/bin/codex-code-mode-host')
    parser.add_argument('--native-cli', type=Path, default=Path.home() / '.local/lib/xcoder/bin/codex')
    parser.add_argument('--blocks', type=int, default=3)
    parser.add_argument('--repeats', type=int, default=5)
    parser.add_argument('--cli-trials', type=int, default=5)
    parser.add_argument('--cli-cells', type=int, default=4)
    parser.add_argument('--retention-cells', type=int, default=100)
    parser.add_argument('--native-mode', choices=('source', 'plan'), default='source',
                        help='source measures custom-cell compilation/cache; plan measures the precompiled ordinary-call path')
    parser.add_argument('--smoke', action='store_true')
    parser.add_argument('--host-only', action='store_true', help='Measure the executor hosts without CLI trials')
    parser.add_argument('--diagnose-json', action='store_true')
    parser.add_argument('--sandbox-regression-label', choices=('before', 'after'),
                        help='Run the focused 20x cold/warm/JSON-plan regression benchmark')
    args = parser.parse_args()
    if args.sandbox_regression_label:
        sandbox_regression_benchmark(args)
        return
    if args.diagnose_json:
        args.native_host = args.native_host.resolve(strict=True)
        args.javascript_host = args.javascript_host.resolve(strict=True)
        json_diagnostic(args)
        return
    if not args.host_only and args.javascript_cli is None:
        parser.error('--javascript-cli is required for full CLI trials')
    binary_keys = ('javascript_host', 'native_host') if args.host_only else ('javascript_host', 'javascript_cli', 'native_host', 'native_cli')
    for key in binary_keys:
        setattr(args, key, getattr(args, key).resolve(strict=True))
    if args.smoke:
        args.blocks, args.repeats, args.cli_trials, args.cli_cells, args.retention_cells = 1, 1, 1, 1, 0
    if min(args.blocks, args.repeats, args.cli_trials, args.cli_cells) < 1 or args.retention_cells < 0:
        parser.error('counts must be positive (retention may be zero)')
    subprocess.run(['git', 'check-ignore', '-q', 'backup/runtime-benchmark'], cwd=ROOT, check=True)
    parent = ROOT / 'backup/runtime-benchmark'
    parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.nice(19)
    report = dict(started_at=time.time(), revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                  workspace_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)),
                  harness_sha256=sha(__file__),
                  kernel=platform.release(), machine=platform.machine(), logical_cpus=os.cpu_count(),
                  cpu_model=next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines()
                                  if line.startswith('model name')), platform.machine()),
                  load_start=os.getloadavg(), sampling_interval_ms=20,
                  hashes={key: sha(getattr(args, key)) for key in binary_keys},
                  methodology=dict(blocks=args.blocks, repeats=args.repeats, cli_trials=0 if args.host_only else args.cli_trials, cli_cells=args.cli_cells,
                                   retention_cells=args.retention_cells, nice=19, cold_filesystem_cache=False,
                                   host_only=args.host_only,
                                   native_mode=args.native_mode,
                                   analytics_enabled=False,
                                   memory='sum of proportional RAM of process tree, including compilers and tool children',
                                   memory_components='anonymous PSS separates heap/stack from file-backed code shared with existing owner sessions',
                                   latency='wall time including compilation, sandbox creation, IPC and output; no LLM inference',
                                   workloads='same results and callback counts; alternating order; private HOME; offline fixture'),
                  host=[], cli=[])
    output = parent / ('smoke.json' if args.smoke else 'results.json')
    if args.host_only:
        output = parent / ('host-' + args.native_mode + ('-smoke' if args.smoke else '') + '.json')
    def save():
        output.write_text(json.dumps(report, indent=2) + '\n')
        output.chmod(0o600)
    save()
    # Unix socket names have a 108-byte ceiling. Keep disposable fixture paths
    # short while retaining the durable measurements in runtime-benchmark/.
    with tempfile.TemporaryDirectory(dir=ROOT / 'backup', prefix='rb-') as temp:
        fixture = Path(temp)
        for block in range(args.blocks):
            for engine in (('javascript', 'native') if block % 2 == 0 else ('native', 'javascript')):
                directory = fixture / ('host-' + engine + '-' + str(block)); directory.mkdir()
                print('Host block', block + 1, engine, flush=True)
                value = host_trial(engine, getattr(args, engine + '_host'), directory, args.repeats,
                                   args.retention_cells if block == 0 else 0, args.native_mode)
                report['host'].append(value); save()
                print('Host completed', engine, value['first_cell'], flush=True)
        for trial in range(0 if args.host_only else args.cli_trials):
            for engine in (('native', 'javascript') if trial % 2 == 0 else ('javascript', 'native')):
                directory = fixture / ('cli-' + engine + '-' + str(trial)); directory.mkdir()
                print('CLI trial', trial + 1, engine, flush=True)
                value = cli_trial(engine, getattr(args, engine + '_cli'), args.javascript_host, directory, args.cli_cells, args.native_mode)
                report['cli'].append(value); save()
                print('CLI completed', engine, round(value['total_ms'], 1), 'ms', flush=True)
    report['finished_at'] = time.time()
    report['load_end'] = os.getloadavg()
    report['summary'] = {}
    for engine in ('javascript', 'native'):
        host = [x for x in report['host'] if x['engine'] == engine]
        cli = [x for x in report['cli'] if x['engine'] == engine]
        report['summary'][engine] = dict(
            host_cell_ms={name: stats([v['ms'] for x in host for v in x['cases'][name]]) for name in host[0]['cases']},
            host_opened_pss_kib=stats([x['opened_pss_kib'] for x in host]),
            host_final_pss_kib=stats([x['final_pss_kib'] for x in host]),
            host_sampled_peak_pss_kib=max(v['pss_kib'] for x in host for v in x['peaks'].values()))
        if cli:
            report['summary'][engine].update(dict(
            cli_total_ms=stats([x['total_ms'] for x in cli]),
            cli_completion_to_exit_ms=stats([x['completion_to_exit_ms'] for x in cli]),
            cli_first_cell_ms=stats([x['cell_ms'][0] for x in cli]),
            cli_later_cell_ms=stats([v for x in cli for v in x['cell_ms'][1:]]) if args.cli_cells > 1 else None,
            cli_post_call_pss_kib=stats([v for x in cli for v in x['post_call_pss_kib']]),
            cli_post_call_pss_anon_kib=stats([v for x in cli for v in x['post_call_pss_anon_kib']]),
            cli_sampled_peak_pss_anon_kib=stats([x['sampled_peak']['pss_anon_kib'] for x in cli]),
            cli_sampled_peak_pss_kib=stats([x['sampled_peak']['pss_kib'] for x in cli])))
    save()
    print(json.dumps(report['summary'], indent=2))
    print('Saved', output, flush=True)


if __name__ == '__main__':
    main()
