#!/usr/bin/env python3
"""Run short real turns on a model: a plain answer, one shell call and one patch edit per effort.

Usage: model-turns.py BINARY WORKDIR [--model M] [--efforts low,medium,...] [--codex-home DIR]
Each turn runs in its own folder under WORKDIR with the sandbox bypassed (agent shells
cannot start bubblewrap). A turn is ACCEPTED only when it exits 0, has no error event or
error item (a refusal such as {"detail":"Bad Request"} or "Model metadata ... not found"
fails it) and has the expected effect. Catalog refresh timeouts are counted separately:
they mean the host's DNS or network is slow, not that the model failed.
Prints one line per turn; never prints credentials (only event types and short messages).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('binary')
p.add_argument('workdir')
p.add_argument('--model')
p.add_argument('--efforts', default='low,medium,high,xhigh')
p.add_argument('--codex-home')
p.add_argument('--timeout', type=int, default=420)
p.add_argument('--only', default='plain,shell,patch')
args = p.parse_args()

env = dict(os.environ)
if args.codex_home:
    env['CODEX_HOME'] = args.codex_home
for name in [k for k in env if k.startswith('RUST_LOG')]:
    del env[name]


def run_turn(cwd, effort, prompt):
    cmd = [args.binary, 'exec', '--json', '--skip-git-repo-check', '--ephemeral',
           '--dangerously-bypass-approvals-and-sandbox', '-C', str(cwd)]
    if args.model:
        cmd += ['-m', args.model]
    if effort:
        cmd += ['-c', f'model_reasoning_effort="{effort}"']
    cmd.append(prompt)
    start = time.time()
    try:
        proc = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True, timeout=args.timeout, stdin=subprocess.DEVNULL)
    except subprocess.TimeoutExpired:
        return dict(rc='timeout', secs=round(time.time() - start), events=[], model=None, text='', errors=['timeout'])
    events, errors, model, text = [], [], None, ''
    for line in proc.stdout.splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        kind = ev.get('type', '')
        item = ev.get('item') or {}
        if kind.startswith('item.') and item:
            events.append(f"{kind}:{item.get('type')}")
            if item.get('type') == 'agent_message' and kind == 'item.completed':
                text = (item.get('text') or '')[:160]
            if item.get('type') == 'error' and kind == 'item.completed':
                errors.append('error item: ' + (item.get('message') or json.dumps(item))[:300])
        else:
            events.append(kind)
        if kind in ('error', 'turn.failed'):
            errors.append(json.dumps(ev)[:400])
        if kind == 'thread.started' or 'model' in ev:
            model = ev.get('model') or model
    catalog_timeouts = 0
    for line in proc.stderr.splitlines():
        low = line.lower()
        if 'failed to refresh available models: request timed out' in low:
            catalog_timeouts += 1
        elif 'error' in low or 'bad request' in low or 'unsupported' in low or 'model' in low:
            if 'token' not in low and 'bearer' not in low:
                errors.append(line[:300])
    return dict(rc=proc.returncode, secs=round(time.time() - start), events=events, model=model,
                text=text, errors=errors[:6], catalog_timeouts=catalog_timeouts)


root = Path(args.workdir)
root.mkdir(parents=True, exist_ok=True)
results = []
for effort in [e for e in args.efforts.split(',') if e]:
    for kind in args.only.split(','):
        cwd = root / f'{effort}-{kind}'
        shutil.rmtree(cwd, ignore_errors=True)
        cwd.mkdir(parents=True)
        if kind == 'plain':
            r = run_turn(cwd, effort, 'Reply with exactly the word PONG and nothing else. Do not run any tools.')
            ok = r['rc'] == 0 and 'PONG' in r['text'] and not r['errors']
            detail = f"reply={r['text']!r}"
        elif kind == 'shell':
            token = f'sol62-{effort}-{int(time.time())}'
            (cwd / 'seed.txt').write_text(token + '\n')
            r = run_turn(cwd, effort, 'Run exactly one shell command, `cat seed.txt`, and then reply with only the text it printed.')
            ran = any(e.endswith(':command_execution') for e in r['events'])
            ok = r['rc'] == 0 and ran and token in r['text'] and not r['errors']
            detail = f"command_execution={ran} reply={r['text']!r}"
        else:
            (cwd / 'notes.txt').write_text('first line\ncolor: alpha\nlast line\n')
            r = run_turn(cwd, effort, 'Use the apply_patch tool (not a shell command) to change "color: alpha" to "color: omega" in notes.txt. Make no other change. Then reply DONE.')
            content = (cwd / 'notes.txt').read_text()
            patched = any(e.endswith(':file_change') for e in r['events'])
            file_ok = content == 'first line\ncolor: omega\nlast line\n'
            ok = r['rc'] == 0 and patched and file_ok and not r['errors']
            detail = f"file_change={patched} file_ok={file_ok}"
        status = 'ACCEPTED' if ok else 'FAILED'
        line = f"{effort:6} {kind:5} {status} rc={r['rc']} {r['secs']}s {detail}"
        if r.get('catalog_timeouts'):
            line += f" (catalog refresh timeouts: {r['catalog_timeouts']})"
        if not ok:
            line += f" events={sorted(set(r['events']))} errors={r['errors']}"
        print(line, flush=True)
        results.append(ok)
print(f"summary: {sum(results)}/{len(results)} accepted", flush=True)
sys.exit(0 if all(results) else 1)
