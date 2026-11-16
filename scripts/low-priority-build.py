#!/usr/bin/env python3
"""Run project builds at the lowest CPU and I/O priority.

Memory is left to the caller's environment (for example a build queue that
sizes and caps each run); this wrapper adds no RAM limit of its own.
"""
import os
import subprocess
import sys


def main():
    if len(sys.argv) < 2:
        raise SystemExit('Supply the build command and its arguments')
    os.setpriority(os.PRIO_PROCESS, 0, 19)
    os.environ.setdefault('CARGO_BUILD_JOBS', '1')
    subprocess.run(['ionice', '-c', '2', '-n', '7', '-p', str(os.getpid())], check=True)
    print('Build resources: nice 19, best-effort I/O priority 7', file=sys.stderr, flush=True)
    os.execvp(sys.argv[1], sys.argv[1:])


if __name__ == '__main__':
    main()
