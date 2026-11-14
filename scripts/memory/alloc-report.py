#!/usr/bin/env python3
"""Summarize an alloc-trace report: live bytes grouped by the first project frames of each stack."""
import argparse
from collections import defaultdict
from pathlib import Path
import re
import subprocess

SKIP = re.compile(r'^(alloc|core|std|hashbrown|serde|serde_json|bytes|tokio|futures|smallvec|indexmap|__rust|__rdl|__rg|_|<alloc|<core|<std|<serde|<bytes|<&|<\[|<\(|<T as|<str|<String|<Vec)')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    parser.add_argument('--symbols', type=Path, required=True, help='unstripped binary matching the traced one')
    parser.add_argument('--top', type=int, default=25)
    parser.add_argument('--frames', type=int, default=6)
    parser.add_argument('--full', action='store_true', help='print whole symbolized stacks')
    args = parser.parse_args()
    base, stacks = None, []
    for line in args.trace.read_text().splitlines():
        fields = line.split()
        if fields[0] == 'M' and base is None and len(fields) > 6 and fields[6].rsplit('/', 1)[-1] in ('xcoder', 'xcode', 'codex'):
            base = int(fields[1].split('-')[0], 16) - int(fields[3], 16)
        elif fields[0] == 'I':
            print('allocator:', ', '.join(f'{name} {int(value) / 1048576:.1f} MiB' for name, value in
                                          (field.split('=') for field in fields[1:])))
        elif fields[0] == 'S':
            stacks.append((int(fields[1]), int(fields[2]), int(fields[3]), [int(value, 16) for value in fields[4:]]))
    stacks.sort(reverse=True)
    total = sum(stack[0] for stack in stacks)
    addresses = sorted({address - 1 - base for stack in stacks[:400] for address in stack[3] if address > base})
    result = subprocess.run(['addr2line', '-f', '-C', '-e', str(args.symbols), *map(hex, addresses)],
                            capture_output=True, text=True, check=True).stdout.splitlines()
    names = {address: result[index * 2] for index, address in enumerate(addresses)}
    print(f'{total / 1048576:.1f} MiB live in tracked allocations, {len(stacks)} stacks')
    groups = defaultdict(lambda: [0, 0])
    for size, count, _, frames in stacks[:400]:
        symbols = [names.get(address - 1 - base, '?') for address in frames]
        if args.full:
            key = '\n      '.join(symbols)
        else:
            key = ' < '.join([name for name in symbols if not SKIP.match(name)][:args.frames])
        groups[key][0] += size
        groups[key][1] += count
    for key, (size, count) in sorted(groups.items(), key=lambda item: -item[1][0])[:args.top]:
        print(f'{size / 1048576:8.2f} MiB {count:6d}  {key}')


if __name__ == '__main__':
    main()
