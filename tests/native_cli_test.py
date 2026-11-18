"""Run the real CLI, native host and real shell tool against an offline model fixture."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import json
import os
import shlex
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]
CLI = Path(os.environ.get('XCODE_TEST_CLI', ROOT / 'build/bin/xcode'))


def fixture_config(port):
    return f'''model = "gpt-6.1-sol"
model_provider = "fixture"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
[analytics]
enabled = false
[features]
code_mode = true
code_mode_only = true
enable_request_compression = false
[model_providers.fixture]
name = "Offline fixture"
base_url = "http://127.0.0.1:{port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
'''


class NativeCliTest(unittest.TestCase):
    def test_agent_loop_uses_native_cell_and_real_approved_tool_path(self):
        requests = []
        failures = []
        memory = {}
        process_ready = threading.Event()
        process_state = {}
        payloads = {}
        calls = []
        # Each shell command stays below Linux's 128 KiB argument limit;
        # the combined plan still exercises a large, fragmented tool input.
        sizes = [4095, 4096, 4097, 16383, 16384, 16385,
                 65535, 65536, 65537, 120000, 120001, 120002,
                 120003, 120004]
        header = 'literal Unicode 雪🙂 quotes "double" \\\\backslash heredoc END\n'
        fill = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ[]{}()<>/\\\\\"' heredoc-like\n"
        for index, size in enumerate(sizes):
            text = header + (fill * ((size - len(header.encode()) + len(fill.encode()) - 1)
                                     // len(fill.encode())))
            encoded = text.encode()[:size]
            encoded = encoded[:-1] + b'\n'
            text = encoded.decode()
            name = f'integrity-{size}.txt'
            payloads[name] = encoded
            delimiter = f'XCODE_INTEGRITY_{index}_EOF'
            command = (f"cat > {name} <<'{delimiter}'\n{text}{delimiter}\n"
                       f"sha256sum {name}")
            calls.append(dict(tool='exec_command',
                              input=dict(cmd=command, max_output_tokens=1000)))
        source = json.dumps(dict(calls=calls), ensure_ascii=False, separators=(',', ':'))
        self.assertLess(len(source.encode()), 1024 * 1024)
        corrupt_source = source[4096:] + source[-4096:]

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.send_error(404)

            def do_POST(self):
                try:
                    body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                    requests.append(json.loads(body))
                    count = len(requests)
                    if count == 2:
                        process_ready.wait()
                        process_name = Path(f'/proc/{process_state["pid"]}/comm').read_text().strip()
                        if process_name != 'xcode':
                            raise AssertionError(f'Xcode process is named {process_name!r}, not xcode')
                        pending = [process_state['pid']]
                        while pending:
                            pid = pending.pop()
                            try:
                                executable = Path(os.readlink(f'/proc/{pid}/exe')).name
                                if executable.startswith('codex-code-mode-host'):
                                    raise AssertionError('ordinary native tool calls started a separate tool host')
                                for task in Path(f'/proc/{pid}/task').iterdir():
                                    pending.extend(int(child) for child in (task / 'children').read_text().split())
                            except (FileNotFoundError, PermissionError):
                                continue
                        for filename, fields in [('status', ['VmRSS', 'VmHWM', 'VmSwap']),
                                                 ('smaps_rollup', ['Rss', 'Pss'])]:
                            try:
                                for line in Path(f'/proc/{process_state["pid"]}/{filename}').read_text().splitlines():
                                    parts = line.split()
                                    if parts[0].rstrip(':') in fields:
                                        memory[parts[0].rstrip(':') + '_KiB'] = int(parts[1])
                            except (FileNotFoundError, PermissionError):
                                pass
                    if count > 2:
                        raise AssertionError('fixture received an unexpected additional model request')
                    events = [dict(type='response.created',
                                   response=dict(id='fixture-' + str(count)))]
                    if count == 1:
                        events.append(dict(
                            type='response.output_item.added',
                            item=dict(id='ctc-native-e2e', type='custom_tool_call',
                                      call_id='native-e2e', name='exec', input='')))
                        at = 0
                        for chunk_size in [1, 7, 31, 127, 1021, 4093, 16381, 65521]:
                            if at >= len(source):
                                break
                            events.append(dict(
                                type='response.custom_tool_call_input.delta',
                                item_id='ctc-native-e2e', call_id='native-e2e',
                                delta=source[at:at + chunk_size]))
                            at += chunk_size
                        while at < len(source):
                            events.append(dict(
                                type='response.custom_tool_call_input.delta',
                                item_id='ctc-native-e2e', call_id='native-e2e',
                                delta=source[at:at + 65521]))
                            at += 65521
                        events.extend([
                            dict(type='response.custom_tool_call_input.done',
                                 item_id='ctc-native-e2e', call_id='native-e2e', input=source),
                            dict(type='response.output_item.done',
                                 item=dict(id='ctc-native-e2e', type='custom_tool_call',
                                           call_id='native-e2e', name='exec',
                                           input=corrupt_source)),
                        ])
                    else:
                        events.append(dict(
                            type='response.output_item.done',
                            item=dict(type='message', role='assistant', id='native-finished',
                                      content=[dict(type='output_text',
                                                    text='native fixture complete')])))
                    events.append(dict(type='response.completed',
                                       response=dict(id='fixture-' + str(count),
                                                     usage=dict(input_tokens=0, output_tokens=0,
                                                                total_tokens=0))))
                    payload = ''.join('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n'
                                      for event in events).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    failures.append(error)
                    self.send_error(500)

        parent = ROOT / 'build/native-cli-tests'
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            directory = Path(temporary)
            home = directory / 'home'
            config = home / '.codex'
            config.mkdir(parents=True)
            server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            server.daemon_threads = True
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            port = server.server_address[1]
            (config / 'config.toml').write_text(f'''model = "gpt-6.1-sol"
model_provider = "fixture"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
[analytics]
enabled = false
[features]
code_mode = true
code_mode_only = true
enable_request_compression = false
[model_providers.fixture]
name = "Offline fixture"
base_url = "http://127.0.0.1:{port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
''')
            env = {key: value for key, value in os.environ.items() if key in ['PATH', 'LANG', 'LC_ALL', 'XCODE_ZIRAN_TOOLCHAIN']}
            env.update(HOME=str(home), CODEX_HOME=str(config), XDG_CONFIG_HOME=str(home / '.config'),
                       XDG_CACHE_HOME=str(directory / 'cache'), XCODE_RUNTIME_DIR=str(directory / 'runtime'),
                       NO_PROXY='127.0.0.1,localhost', RUST_LOG='warn',
                       XCODE_ZIRAN_TOOLCHAIN=str(directory / 'no-compiler'),
                       HARMONY_SOURCE_SESSION='xcode-offline-local-test', HARMONY_MODEL_INPUT_DISABLE='1', XCODE_UPSTREAM_VERSION_CHECK='0')
            try:
                command = [str(CLI), 'exec', '--ephemeral', '--skip-git-repo-check',
                           '--sandbox', 'workspace-write', '--json', 'Run the native local fixture.']
                process = subprocess.Popen(command, cwd=directory, env=env, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, text=True)
                process_state['pid'] = process.pid
                process_ready.set()
                try:
                    stdout, stderr = process.communicate(timeout=180)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.communicate()
                    raise
                result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
            if failures:
                raise failures[0]
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(len(requests), 2, result.stdout + result.stderr)
            request_record = ROOT / 'build/native-cli-request.json'
            request_record.write_text(json.dumps(requests[0], indent=2) + '\n')
            request_record.chmod(0o600)
            def exec_definitions(definitions):
                for definition in definitions:
                    if definition.get('name') in ['exec', 'functions.exec']:
                        yield definition
                    yield from exec_definitions(definition.get('tools', []))
            definitions = list(requests[0].get('tools', []))
            # The current Responses transport places tool configuration among
            # input items; older transports used a top-level tools array.
            for item in requests[0].get('input', []):
                definitions.extend(item.get('tools', []))
            advertised = list(exec_definitions(definitions))
            self.assertEqual(len(advertised), 1, json.dumps(definitions))
            self.assertIn('precompiled native Ziran executor', advertised[0]['description'])
            outputs = [item for item in requests[-1]['input'] if item.get('type') == 'custom_tool_call_output'
                       and item.get('call_id') == 'native-e2e']
            self.assertEqual(len(outputs), 1, json.dumps(requests[-1]))
            output = json.dumps(outputs[0]['output'])
            self.assertNotIn('unknown or ambiguous enabled tool', output)
            self.assertNotIn('compilation failed', output)
            self.assertIn('native fixture complete', result.stdout)
            for name, expected in payloads.items():
                actual = (directory / name).read_bytes()
                self.assertEqual(hashlib.sha256(actual).hexdigest(),
                                 hashlib.sha256(expected).hexdigest(), name)
                self.assertEqual(actual, expected, name)
            (ROOT / 'build/native-cli-metrics.json').write_text(json.dumps(
                dict(fixture='offline native tool loop', executable=str(CLI), separate_tool_hosts=0, **memory), indent=2) + '\n')


    def test_shell_apply_patch_runs_xcode_helper_ahead_of_login_profile(self):
        # Regression: a compound `apply_patch` heredoc is not intercepted, so the
        # shell resolves `apply_patch` itself. Login startup reset PATH and put a
        # profile copy first; that copy dropped each added file's first line and
        # wrote the rest twice. Xcode's own helper must win.
        files = {
            'Makefile': 'ZIRAN ?= ./scripts/ziran.sh\nSOURCES := src/app.zi\n\nall:\n\t$(ZIRAN) build\n',
            'README.md': '# taskmgr\n\nA task manager for Linux.\n',
            '.gitignore': 'build/\nziran.local.toml\n',
            'scripts/tool.sh': '#!/bin/sh\necho ok\n',
        }
        patch = '*** Begin Patch\n' + ''.join(
            f'*** Add File: {name}\n' + ''.join('+' + line + '\n' for line in text.splitlines())
            for name, text in files.items()) + '*** End Patch\n'
        source = json.dumps(dict(calls=[
            dict(tool='exec_command', input=dict(
                cmd=f"mkdir -p out && cd out && apply_patch <<'PATCH'\n{patch}PATCH\nchmod +x scripts/tool.sh",
                max_output_tokens=1000)),
            dict(tool='exec_command', input=dict(cmd='command -v apply_patch', max_output_tokens=1000)),
        ]))
        requests = []
        failures = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    requests.append(json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0)))))
                    count = len(requests)
                    if count > 2:
                        raise AssertionError('fixture received an unexpected additional model request')
                    item = (dict(id='ctc-apply-patch', type='custom_tool_call', call_id='apply-patch-path',
                                 name='exec', input=source) if count == 1 else
                            dict(type='message', role='assistant', id='apply-patch-finished',
                                 content=[dict(type='output_text', text='apply_patch fixture complete')]))
                    events = [dict(type='response.created', response=dict(id=f'fixture-{count}')),
                              dict(type='response.output_item.done', item=item),
                              dict(type='response.completed', response=dict(
                                  id=f'fixture-{count}',
                                  usage=dict(input_tokens=0, output_tokens=0, total_tokens=0)))]
                    payload = ''.join('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n'
                                      for event in events).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except BaseException as error:
                    failures.append(error)
                    self.send_error(500)

        parent = ROOT / 'build/native-cli-tests'
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            directory = Path(temporary)
            home = directory / 'home'
            config = home / '.codex'
            config.mkdir(parents=True)
            profile_bin = home / '.local/bin'
            profile_bin.mkdir(parents=True)
            marker = home / 'profile-apply-patch-used'
            shim = profile_bin / 'apply_patch'
            shim.write_text(f'#!/bin/sh\ncat >/dev/null\ntouch {shlex.quote(str(marker))}\n')
            shim.chmod(0o755)
            for name in ['.bash_profile', '.profile', '.zprofile']:
                (home / name).write_text('export PATH="$HOME/.local/bin:$PATH"\n')
            server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            server.daemon_threads = True
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            (config / 'config.toml').write_text(fixture_config(server.server_address[1]))
            env = {key: value for key, value in os.environ.items() if key in ['PATH', 'LANG', 'LC_ALL']}
            env.update(HOME=str(home), CODEX_HOME=str(config), XDG_CONFIG_HOME=str(home / '.config'),
                       XDG_CACHE_HOME=str(directory / 'cache'), XCODE_RUNTIME_DIR=str(directory / 'runtime'),
                       NO_PROXY='127.0.0.1,localhost', RUST_LOG='warn', SHELL='/bin/bash',
                       XCODE_ZIRAN_TOOLCHAIN=str(directory / 'no-compiler'),
                       HARMONY_SOURCE_SESSION='xcode-offline-local-test', HARMONY_MODEL_INPUT_DISABLE='1', XCODE_UPSTREAM_VERSION_CHECK='0')
            try:
                result = subprocess.run(
                    [str(CLI), 'exec', '--ephemeral', '--skip-git-repo-check', '--sandbox', 'workspace-write',
                     '--json', 'Run the apply_patch fixture.'],
                    cwd=directory, env=env, capture_output=True, text=True, timeout=180)
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
            if failures:
                raise failures[0]
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(len(requests), 2, result.stdout + result.stderr)
            outputs = [item for item in requests[-1]['input'] if item.get('type') == 'custom_tool_call_output'
                       and item.get('call_id') == 'apply-patch-path']
            self.assertEqual(len(outputs), 1, json.dumps(requests[-1]))
            output = json.dumps(outputs[0]['output'])
            self.assertFalse(marker.exists(), 'login profile apply_patch ran instead of xcode: ' + output)
            self.assertNotIn(str(shim), output)
            for name, text in files.items():
                self.assertEqual((directory / 'out' / name).read_bytes(), text.encode(), name)
            self.assertTrue(os.access(directory / 'out/scripts/tool.sh', os.X_OK))


if __name__ == '__main__':
    unittest.main()
