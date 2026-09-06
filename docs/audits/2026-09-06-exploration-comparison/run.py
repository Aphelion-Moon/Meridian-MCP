"""Experimental Windows replay of the recorded exploration study, not a CI gate.

Only starts an isolated analysis-mode MCP and reads the specified checkout.
Raw replies stay in the explicitly selected output directory.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import threading
import time

BASE = Path(__file__).resolve().parent


def encode(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical_output(operation, text, rift, fixture):
    def portable(value):
        if isinstance(value, dict):
            return {key: portable(item) for key, item in value.items() if key != 'state_generation'}
        if isinstance(value, list):
            return [portable(item) for item in value]
        if isinstance(value, str):
            value = value.removeprefix('\\\\?\\')
            for root, label in [(fixture, '<fixture>'), (rift, '<rift>')]:
                value = value.replace(str(root), label).replace(root.as_posix(), label)
            return value
        return value
    if operation['arm'] == 'mcp':
        try:
            return json.dumps(portable(json.loads(text)), sort_keys=True, ensure_ascii=False)
        except json.JSONDecodeError:
            return text
    if 'rg' in operation:
        return '\n'.join(sorted(re.sub(r'^\.[\\/]', '', line) for line in text.splitlines()))
    return text


def explicit_rg_args(arguments):
    # Without an explicit path, rg may search empty piped stdin during replay.
    if '--files' in arguments:
        return arguments
    positionals = []
    skip = False
    for argument in arguments:
        if skip:
            skip = False
        elif argument in ('-g', '--glob', '-A', '-B', '-C', '--max-count'):
            skip = True
        elif not argument.startswith('-'):
            positionals.append(argument)
    return [*arguments, '.'] if len(positionals) == 1 else arguments


class Counters(ctypes.Structure):
    _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD)] + [
        (name, ctypes.c_size_t) for name in (
            'PeakWorkingSetSize', 'WorkingSetSize', 'QuotaPeakPagedPoolUsage',
            'QuotaPagedPoolUsage', 'QuotaPeakNonPagedPoolUsage',
            'QuotaNonPagedPoolUsage', 'PagefileUsage', 'PeakPagefileUsage', 'PrivateUsage')]


def memory(process):
    counters = Counters()
    counters.cb = ctypes.sizeof(counters)
    call = ctypes.windll.psapi.GetProcessMemoryInfo
    call.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    if not call(wintypes.HANDLE(int(process._handle)), ctypes.byref(counters), counters.cb):
        raise ctypes.WinError()
    return {key: getattr(counters, key) for key in ('WorkingSetSize', 'PeakWorkingSetSize', 'PrivateUsage')}


class Server:
    def __init__(self, binary, rift, fixture):
        environment = {key: value for key, value in os.environ.items() if not key.startswith('MERIDIAN_MCP_')}
        environment.update(MERIDIAN_MCP_MODE='analysis', MERIDIAN_MCP_ROOTS=os.pathsep.join(map(str, (rift, fixture))))
        self.process = subprocess.Popen([str(binary)], cwd=rift, env=environment,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            encoding='utf-8', bufsize=1)
        self.lines = queue.Queue()
        self.stderr = []
        self.index = 0
        threading.Thread(target=lambda: [self.lines.put(line) for line in self.process.stdout], daemon=True).start()
        threading.Thread(target=lambda: self.stderr.extend(self.process.stderr.readlines()), daemon=True).start()

    def initialize(self):
        self.rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
            'clientInfo': {'name': 'exploration-comparison-replay', 'version': '1'}})
        self.process.stdin.write(encode({'jsonrpc': '2.0', 'method': 'notifications/initialized', 'params': {}}) + '\n')
        self.process.stdin.flush()

    def rpc(self, method, parameters):
        self.index += 1
        request = {'jsonrpc': '2.0', 'id': self.index, 'method': method, 'params': parameters}
        started = time.perf_counter()
        self.process.stdin.write(encode(request) + '\n')
        self.process.stdin.flush()
        deadline = time.monotonic() + 180
        while True:
            response = json.loads(self.lines.get(timeout=max(0.001, deadline - time.monotonic())))
            if response.get('id') == self.index:
                return response, (time.perf_counter() - started) * 1000
            if time.monotonic() >= deadline:
                raise TimeoutError('MCP response deadline')

    def close(self):
        self.process.stdin.close()
        try:
            code = self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.process.kill()
            code = self.process.wait()
        return {'exit_code': code, 'stderr': ''.join(self.stderr)}


def observe(server, operation, rift, fixture):
    if operation['arm'] == 'mcp':
        arguments = {key: (value.replace('$RIFT', str(rift)).replace('$FIXTURE', str(fixture))
            if isinstance(value, str) else value) for key, value in operation.get('args', {}).items()}
        response, elapsed = server.rpc('tools/call', {'name': operation['tool'], 'arguments': arguments})
        result = response.get('result', {})
        text = '\n'.join(item.get('text', '') for item in result.get('content', []))
        if 'error' in response:
            text = encode(response['error'])
        failed = bool(response.get('error') or result.get('isError'))
    else:
        root = fixture if operation.get('root') == 'fixture' else rift
        started = time.perf_counter()
        if 'rg' in operation:
            result = subprocess.run(['rg', *explicit_rg_args(operation['rg'])], cwd=root,
                stdin=subprocess.DEVNULL, capture_output=True, encoding='utf-8', errors='replace', timeout=60)
            text, failed = result.stdout + result.stderr, result.returncode not in (0, 1)
        else:
            pieces = []
            for spec in operation['read']:
                path = (root / spec['path']).resolve()
                if not path.is_relative_to(root):
                    raise ValueError('Source read escaped its root')
                lines = path.read_text(encoding='utf-8-sig', errors='replace').splitlines()
                pieces.append(spec['path'] + '\n' + '\n'.join(
                    f'{line}: {lines[line - 1]}' for line in range(spec['start'], min(spec['end'], len(lines)) + 1)))
            text, failed = '\n'.join(pieces), False
        elapsed = (time.perf_counter() - started) * 1000
    return {'operation': operation, 'elapsed_ms': elapsed, 'text_characters': len(text),
        'text_bytes': len(text.encode('utf-8')), 'failed': failed, 'output': text}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--rift-root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path, help='New directory; keep raw results under ignored target/')
    parser.add_argument('--repeats', type=int, default=5, choices=range(1, 11))
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('This recorded experiment and memory counter probe target Windows.')
    binary, rift, fixture = args.binary.resolve(), args.rift_root.resolve(), (BASE / 'fixture').resolve()
    baseline = json.loads((BASE / 'results.json').read_text())
    operations = json.loads((BASE / 'operations.json').read_text())
    expected = json.loads((BASE / 'expected.json').read_text())
    if sha(binary.read_bytes()) != baseline['identity']['binary_sha256']:
        parser.error('Binary differs from this recorded baseline; create a separate candidate study.')
    revision = subprocess.check_output(['git', '-c', f'safe.directory={rift.as_posix()}', 'rev-parse', 'HEAD'], cwd=rift, text=True).strip()
    if revision != baseline['identity']['rift_head']:
        parser.error('Checkout revision differs from the recorded cases.')

    def verify_sources():
        for relative, digest in baseline['source']['files_sha256'].items():
            if sha((rift / relative).read_bytes()) != digest:
                raise ValueError(f'Changed evidence file: {relative}')
    verify_sources()
    args.output.mkdir(parents=True, exist_ok=False)
    server = Server(binary, rift, fixture)
    results = []
    setup = []
    try:
        server.initialize()
        for prefix, dme in [('R', rift / 'tgstation.dme'), ('S', fixture / 'fixture.dme')]:
            parse = observe(server, {'case': prefix + '_parse', 'arm': 'mcp', 'tool': 'dm_parse_environment', 'args': {'dme_path': str(dme)}}, rift, fixture)
            parse['memory'] = memory(server.process)
            setup.append(parse)
            if parse['failed']:
                raise RuntimeError(parse['output'])
            print(encode({'stage': prefix, 'parse_ms': parse['elapsed_ms'], 'memory': parse['memory']}), flush=True)
            for trial in range(args.repeats):
                reuse = observe(server, {'case': prefix + '_reuse', 'arm': 'mcp', 'tool': 'dm_parse_environment', 'args': {'dme_path': str(dme)}}, rift, fixture)
                setup.append(reuse)
                if not json.loads(reuse['output']).get('reused'):
                    raise RuntimeError('Source changed during replay')
                for case in sorted({op['case'] for op in operations if op['case'].startswith(prefix)}):
                    arms = ('mcp', 'hybrid', 'control') if trial % 2 == 0 else ('control', 'mcp', 'hybrid')
                    for arm in arms:
                        for index, operation in enumerate(operations):
                            if operation['case'] != case or operation['arm'] != arm:
                                continue
                            result = observe(server, operation, rift, fixture)
                            result['trial'] = trial
                            results.append(result)
                            digest = sha(canonical_output(operation, result['output'], rift, fixture).encode('utf-8'))
                            if digest != expected[index]['canonical_sha256'] or result['failed'] != expected[index]['failed']:
                                raise AssertionError(f'Response differs for operation {index}: {case}/{arm}')
            print(encode({'stage': prefix, 'equivalent_replays': args.repeats}), flush=True)
        verify_sources()
    finally:
        closing = server.close()
        (args.output / 'raw.json').write_text(encode({'setup': setup, 'results': results, 'shutdown': closing}), encoding='utf-8')
        print(encode({'shutdown': closing}), flush=True)
    if closing['exit_code'] != 0:
        raise RuntimeError('MCP did not exit cleanly')
    print(encode({'passed': True, 'matched_operations': len(results), 'source_hashes_checked': len(baseline['source']['files_sha256'])}), flush=True)


if __name__ == '__main__':
    main()
