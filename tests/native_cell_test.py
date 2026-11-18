import json
import os
from pathlib import Path
import tempfile
import unittest

from native_host_test import Peer


ROOT = Path(__file__).resolve().parents[1]
ZIRAN = Path(os.environ['XCODE_ZIRAN_TOOLCHAIN']).resolve()


class NativeCellTest(unittest.TestCase):
    def setUp(self):
        parent = ROOT / 'build/native-tests'
        parent.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(self.temp.cleanup)
        self.peer = Peer(Path(self.temp.name), toolchain=ZIRAN)
        self.addCleanup(self.peer.close)
        self.peer.send(dict(type='connection/hello', supportedVersions=[1], requiredCapabilities=[],
                            optionalCapabilities=['bounded-output-v1', 'yield-observation',
                                                  'session-cell-execution-resource-limits']))
        self.assertEqual(self.peer.receive()['type'], 'connection/ready')
        self.peer.operation(1, 'session/open', outputTruncation='server',
                            cellExecutionLimits=dict(maxHeapSizeBytes=268435456,
                                                     maxYieldTimeMs=30000))
        opened = self.peer.receive()
        self.assertEqual(opened['result']['status'], 'ok', opened)

    def run_cell(self, source):
        tools = [dict(name='fixture', tool_name=dict(name='fixture', namespace=None),
                      description='native fixture', kind='function', input_schema=None,
                      output_schema=None)]
        self.peer.operation(2, 'session/execute', request=dict(tool_call_id='native-cell-test',
                            enabled_tools=tools, source=source, yield_time_ms=30000,
                            max_output_tokens=10000))
        started = self.peer.receive()
        self.assertEqual(started['type'], 'operation/response', started)
        self.assertEqual(started['result']['status'], 'ok', started)
        calls = []
        while True:
            message = self.peer.receive()
            if message['type'] == 'delegate/request':
                invocation = message['request']['invocation']
                self.assertEqual(invocation['tool_name']['name'], 'fixture', invocation)
                calls.append(invocation['input'])
                self.peer.send(dict(type='delegate/response', id=message['id'],
                               result=dict(status='ok', value=dict(type='tool/result', result=42))))
                continue
            self.assertEqual(message['type'], 'execute/initialResponse', message)
            result = message['result']['value']['Result']
            self.assertIsNone(result['error_text'], result)
            return calls, [item['text'] for item in result['content_items']]

    def test_real_tools_and_callback_frames(self):
        calls, output = self.run_cell('''using X :: #import "xcode/runtime"
main :: () -> s32 {
    reply := Call("fixture", "{\\"value\\":42}")
    defer Release(*reply)
    if !reply.ok { return 4 }
    Text(Body(*reply))
    return 0
}
''')
        self.assertEqual(calls, [{'value': 42}])
        self.assertEqual(json.loads(output[0]), {'ok': True, 'value': 42})

    def test_owner_and_source_file_contents_are_inaccessible(self):
        calls, output = self.run_cell('''using X :: #import "xcode/runtime"
using F :: #import "std/file"
main :: () -> s32 {
    password, owner_ok := F.ReadEntireFile("/etc/passwd")
    source, source_ok := F.ReadEntireFile("main.zi")
    if owner_ok || source_ok || password.count != 0 || source.count != 0 { return 5 }
    if F.WriteEntireFile("/etc/xcode-sandbox-test", "forbidden") { return 6 }
    Text("isolated")
    return 0
}
''')
        self.assertEqual(calls, [])
        self.assertEqual(output, ['isolated'])

    def test_replies_are_freed_in_a_loop(self):
        source = '''using X :: #import "xcode/runtime"
main :: () -> s32 {
    for i: 0..99 { reply := Call("fixture"); if !reply.ok { return 4 }; Release(*reply) }
    return 0
}
'''
        calls, output = self.run_cell(source)
        self.assertEqual(len(calls), 100)
        self.assertEqual(output, [])

    def test_batch_calls_release_each_reply(self):
        source = '''using X :: #import "xcode/runtime"
main :: () -> s32 {
    calls: [4]BatchCall
    for i: 0..3 { calls[i].name = "fixture"; calls[i].input = "{}" }
    CallBatch(calls[:])
    for i: 0..3 { Text(Body(*calls[i].reply)); Release(*calls[i].reply) }
    return 0
}
'''
        calls, output = self.run_cell(source)
        self.assertEqual(calls, [{}] * 4)
        self.assertEqual([json.loads(item)['value'] for item in output], [42] * 4)


if __name__ == '__main__':
    unittest.main()
