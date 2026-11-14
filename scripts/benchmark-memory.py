#!/usr/bin/env python3
"""Measure how an interactive Xcoder session's memory grows and what an idle session keeps resident.

The session runs offline in a private tmux terminal against a local model
fixture. One typed prompt starts a chain of real shell tool calls, each
returning a marked block of output. The report records resident memory after
each call, the idle footprint by mapping kind, how many copies of each tool
output remain in the process (a direct count of retained duplicates) and,
when the binary releases idle memory, the footprint after that release and the
time to answer the next keypress.
"""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
MARK = 'XOUT_'
ROLLUP = ('Rss', 'Pss', 'Pss_Anon', 'Pss_File', 'Private_Dirty', 'Swap')


def rollup(pid):
    values = {}
    for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
        fields = line.split()
        if fields[0][:-1] in ROLLUP:
            values[fields[0][:-1].lower() + '_kib'] = int(fields[1])
    return values


def by_mapping(pid):
    """Resident and swapped KiB grouped by what backs the pages."""
    groups, kind = {}, None
    for line in Path(f'/proc/{pid}/smaps').read_text().splitlines():
        fields = line.split()
        if re.match(r'^[0-9a-f]+-[0-9a-f]+$', fields[0]):
            name = fields[5] if len(fields) > 5 else ''
            executable = 'x' in fields[1]
            if name == '[heap]':
                kind = 'main heap'
            elif not name:
                kind = 'anonymous (allocator arenas, thread stacks)'
            elif name.startswith('['):
                kind = 'kernel'
            elif '/bin/' in name and ('xcode' in name or 'codex' in name or 'xcoder' in name):
                kind = 'program code' if executable else 'program data and tables'
            elif '.so' in name:
                kind = 'shared libraries'
            else:
                kind = 'other files'
        elif fields[0] in ('Rss:', 'Swap:', 'Private_Dirty:'):
            entry = groups.setdefault(kind, dict(rss_kib=0, swap_kib=0, private_dirty_kib=0))
            entry[fields[0][:-1].lower() + '_kib'] += int(fields[1])
    return groups


def external_release(pid):
    """Prototype of the idle release, driven from outside with a debugger:
    return freed allocator pages, then ask the kernel to page out every private mapping."""
    commands = ['set pagination off', 'call (int)malloc_trim(0)']
    for line in Path(f'/proc/{pid}/maps').read_text().splitlines():
        fields = line.split()
        if fields[1][3] != 'p' or (len(fields) > 5 and fields[5] in ('[vvar]', '[vdso]', '[vsyscall]', '[vvar_vclock]')):
            continue
        start, end = (int(value, 16) for value in fields[0].split('-'))
        commands.append(f'call (int)madvise((void*){start}, {end - start}, 21)')
    script = Path(f'/proc/{pid}/cwd').resolve().parent / 'release.gdb'
    script.write_text('\n'.join(commands) + '\n')
    started = time.monotonic()
    subprocess.run(['gdb', '-q', '-batch', '-p', str(pid), '-x', str(script)], capture_output=True, text=True)
    return time.monotonic() - started


def marker_copies(pid, calls, context=None, contexts=None):
    """Count occurrences of every call's marker in writable private memory.

    With `context`, also collect the bytes surrounding that call's copies; they
    show which structure (JSON event, terminal cell, raw buffer) holds each one.
    """
    counts = [0] * calls
    pattern = re.compile((MARK + r'(\d{5})_').encode())
    try:
        with open(f'/proc/{pid}/mem', 'rb', buffering=0) as memory:
            for line in Path(f'/proc/{pid}/maps').read_text().splitlines():
                fields = line.split()
                if not fields[1].startswith('rw') or (len(fields) > 5 and not fields[5].startswith('[heap')):
                    continue
                start, end = (int(value, 16) for value in fields[0].split('-'))
                position = start
                while position < end:
                    size = min(end - position, 8 << 20)
                    try:
                        memory.seek(position)
                        block = memory.read(size)
                    except OSError:
                        block = b''
                    for match in pattern.finditer(block):
                        index = int(match.group(1))
                        if index < calls:
                            counts[index] += 1
                            if index == context:
                                before = block[max(match.start() - 700, 0):match.start()]
                                after = block[match.end():match.end() + 60]
                                contexts.append(dict(address=hex(position + match.start()),
                                                     before=before.decode('latin-1'), after=after.decode('latin-1')))
                    position += size
    except PermissionError:
        return None
    return counts


class Terminal:
    """A private tmux server kept as a child so its session's memory is readable."""

    def __init__(self, directory):
        self.name = 'xcoder-memory-' + str(os.getpid())
        environment = {key: os.environ[key] for key in ('PATH', 'LANG', 'LC_ALL', 'HOME') if key in os.environ}
        environment['TMUX_TMPDIR'] = str(directory)
        self.environment = environment
        self.server = subprocess.Popen(['tmux', '-L', self.name, '-D'], env=environment,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 10
        while self.run('list-sessions', check=False).returncode and time.monotonic() < deadline:
            if 'no server' not in self.run('list-sessions', check=False).stderr:
                break
            time.sleep(0.05)

    def run(self, *arguments, check=True):
        return subprocess.run(['tmux', '-L', self.name, *arguments], env=self.environment,
                              capture_output=True, text=True, check=check)

    def screen(self):
        return self.run('capture-pane', '-p', '-t', 'memory', check=False).stdout

    def close(self):
        self.run('kill-server', check=False)
        try:
            self.server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.server.kill()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, default=Path.home() / '.local/lib/xcoder/bin/xcode')
    parser.add_argument('--calls', type=int, default=120, help='tool calls in the session')
    parser.add_argument('--output-bytes', type=int, default=30000, help='bytes each tool call prints')
    parser.add_argument('--idle-seconds', type=float, default=20, help='idle time before the idle measurement')
    parser.add_argument('--release-wait', type=float, default=0,
                        help='extra idle seconds to wait for the idle memory release, then measure again')
    parser.add_argument('--context-call', type=int,
                        help='record the memory surrounding every retained copy of this call\'s output')
    parser.add_argument('--external-release', action='store_true',
                        help='after the idle measurement, trim and page out the session with a debugger (prototype reference)')
    parser.add_argument('--environment', action='append', default=[], metavar='NAME=VALUE')
    parser.add_argument('--trace-library', type=Path,
                        help='preload scripts/memory/alloc-trace.c (built as a shared library) and save its live-allocation report')
    parser.add_argument('--trace-output', type=Path, default=ROOT / 'build/memory/alloc-trace.txt')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--save-request', type=Path, help='save the final model request body')
    args = parser.parse_args()
    cli = args.cli.resolve()

    requests, samples, errors, state = [], [], [], {}
    finished = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *arguments):
            pass

        def do_POST(self):
            try:
                body = self.rfile.read(int(self.headers['Content-Length']))
                parsed = json.loads(body)
                metadata = json.loads((parsed.get('client_metadata') or {}).get('x-codex-turn-metadata') or '{}')
                if metadata.get('thread_source') != 'user':
                    # Side threads (for example the title writer) get one short answer.
                    state['side_requests'] = state.get('side_requests', 0) + 1
                    count, final_text = args.calls, 'Memory benchmark'
                else:
                    count, final_text = len(requests), 'memory benchmark complete'
                    requests.append(len(body))
                    state['last_request'] = body
                    if count:
                        samples.append(dict(call=count, request_bytes=len(body), seconds=time.monotonic() - state['started'],
                                            **rollup(state['pid'])))
                if count < args.calls:
                    command = (f"printf '{MARK}%05d_' {count}; head -c {args.output_bytes * 3 // 4} /dev/urandom | base64 -w 120")
                    plan = dict(calls=[dict(tool='exec_command', input=dict(cmd=command, max_output_tokens=args.output_bytes))])
                    items = [dict(type='message', role='assistant', id='note-' + str(count),
                                  content=[dict(type='output_text', text=f'Step {count}: reading the next block of output.')]),
                             dict(type='custom_tool_call', call_id='memory-' + str(count), name='exec', input=json.dumps(plan))]
                else:
                    items = [dict(type='message', role='assistant', id='done',
                                  content=[dict(type='output_text', text=final_text)])]
                events = [dict(type='response.created', response=dict(id='fixture-' + str(count)))]
                events += [dict(type='response.output_item.done', item=item) for item in items]
                events.append(dict(type='response.completed', response=dict(
                    id='fixture-' + str(count), usage=dict(input_tokens=0, output_tokens=0, total_tokens=0))))
                wire = ''.join('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n' for event in events).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(wire)))
                self.end_headers()
                self.wfile.write(wire)
                self.wfile.flush()
                if count >= args.calls and final_text.startswith('memory'):
                    finished.set()
            except BaseException as error:
                errors.append(repr(error))
                self.send_error(500)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()

    (ROOT / 'build/tmp').mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(dir=ROOT / 'build/tmp', prefix='memory-'))
    terminal = None
    try:
        home = directory / 'home'
        work = directory / 'work'
        config = home / '.codex'
        for path in (config, work, directory / 'tmp', directory / 't'):
            path.mkdir(parents=True)
        (config / 'config.toml').write_text(f'''model = "gpt-6.1-sol"
model_provider = "fixture"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
approval_policy = "never"
sandbox_mode = "read-only"
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
[projects."{work}"]
trust_level = "trusted"
''')
        environment = dict(HOME=str(home), CODEX_HOME=str(config), TMPDIR=str(directory / 'tmp'),
                           XDG_CONFIG_HOME=str(home / '.config'), XDG_CACHE_HOME=str(home / '.cache'),
                           XCODER_RUNTIME_DIR=str(directory / 'r'), RUST_LOG='error',
                           NO_PROXY='127.0.0.1,localhost', HARMONY_SOURCE_SESSION='xcoder-offline-benchmark',
                           HARMONY_MODEL_INPUT_DISABLE='1', XCODER_ZIRAN_TOOLCHAIN=str(directory / 'no-compiler'))
        if args.trace_library:
            environment.update(LD_PRELOAD=str(args.trace_library.resolve()),
                               XCODER_ALLOC_TRACE=str(directory / 'alloc-trace.txt'))
        for setting in args.environment:
            name, _, value = setting.partition('=')
            environment[name] = value
        terminal = Terminal(directory / 't')
        command = ['env', '-i', 'PATH=' + os.environ['PATH'], 'TERM=xterm-256color',
                   *[name + '=' + value for name, value in environment.items()], str(cli), '--dangerously-bypass-approvals-and-sandbox']
        terminal.run('new-session', '-d', '-s', 'memory', '-x', '160', '-y', '48', '-c', str(work),
                     ' '.join("'" + part.replace("'", "'\\''") + "'" for part in command) + '; sleep 5')
        pane = int(terminal.run('list-panes', '-t', 'memory', '-F', '#{pane_pid}').stdout.split()[0])
        deadline = time.monotonic() + 60
        pid = None
        while time.monotonic() < deadline:
            children = subprocess.run(['pgrep', '-P', str(pane)], capture_output=True, text=True).stdout.split()
            candidates = [pane, *map(int, children)]
            for candidate in candidates:
                try:
                    if Path(os.readlink(f'/proc/{candidate}/exe')) == cli:
                        pid = candidate
                except OSError:
                    pass
            if pid and '›' in terminal.screen():
                break
            time.sleep(0.2)
        if not pid:
            raise SystemExit('The session did not start:\n' + terminal.screen())
        state['pid'] = pid
        time.sleep(3)
        fresh = dict(**rollup(pid), mappings=by_mapping(pid))
        state['started'] = time.monotonic()
        terminal.run('send-keys', '-t', 'memory', '-l', 'Run the offline memory benchmark.')
        time.sleep(1.5)
        terminal.run('send-keys', '-t', 'memory', 'Enter')
        if not finished.wait(timeout=30 + args.calls * 20):
            raise SystemExit('The session did not finish; errors: ' + repr(errors) + '\n' + terminal.screen())
        active_seconds = time.monotonic() - state['started']
        time.sleep(args.idle_seconds)
        idle = dict(**rollup(pid), mappings=by_mapping(pid))
        contexts = []
        copies = marker_copies(pid, args.calls, args.context_call, contexts)
        report = dict(cli=str(cli), calls=args.calls, output_bytes=args.output_bytes,
                      environment=args.environment, active_seconds=active_seconds,
                      final_request_bytes=requests[-1], side_requests=state.get('side_requests', 0), fresh=fresh, samples=samples, idle=idle,
                      output_copies=None if copies is None else dict(
                          per_call_median=sorted(copies)[len(copies) // 2], minimum=min(copies), maximum=max(copies),
                          total=sum(copies), retained_bytes_estimate=sum(copies) * args.output_bytes))
        if args.context_call is not None:
            report['copy_contexts'] = contexts
        if args.trace_library:
            (directory / 'alloc-trace.txt.trigger').touch()
            deadline = time.monotonic() + 60
            while (directory / 'alloc-trace.txt.trigger').exists() and time.monotonic() < deadline:
                time.sleep(0.2)
            args.trace_output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(directory / 'alloc-trace.txt', args.trace_output)
        if args.external_release:
            report['external_release_seconds'] = external_release(pid)
        if args.release_wait or args.external_release:
            time.sleep(args.release_wait)
            report['released'] = dict(**rollup(pid), mappings=by_mapping(pid))
            started = time.monotonic()
            terminal.run('send-keys', '-t', 'memory', '-l', 'x')
            while 'x' not in terminal.screen().split('›')[-1][:8] and time.monotonic() - started < 30:
                time.sleep(0.005)
            report['wake_ms'] = (time.monotonic() - started) * 1000
            time.sleep(2)
            report['after_wake'] = rollup(pid)
        if args.save_request:
            args.save_request.write_bytes(state['last_request'])
        text = json.dumps(report, indent=2) + '\n'
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(text)
        summary = dict(fresh=fresh['rss_kib'], idle=idle['rss_kib'], idle_anon=idle['pss_anon_kib'],
                       growth_per_call_kib=(idle['pss_anon_kib'] - fresh['pss_anon_kib']) / max(args.calls, 1),
                       final_request_bytes=requests[-1], output_copies=report['output_copies'],
                       idle_mappings=idle['mappings'], released=report.get('released', {}).get('rss_kib'),
                       released_swap=report.get('released', {}).get('swap_kib'), wake_ms=report.get('wake_ms'),
                       after_wake=report.get('after_wake', {}).get('rss_kib'), errors=errors)
        print(json.dumps(summary, indent=2))
    finally:
        if terminal:
            terminal.close()
        server.shutdown()
        server.server_close()
        shutil.rmtree(directory, ignore_errors=True)


if __name__ == '__main__':
    main()
