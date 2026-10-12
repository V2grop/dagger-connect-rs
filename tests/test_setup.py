"""Menu failure regressions and real forwarding through menu-generated configs.

Run after cargo build, or set DAGGER_TEST_BIN to the supplied original binary.
All network tests use loopback. Service tests isolate systemd behind a fixture.
"""
from pathlib import Path
import json
import os
import socket
import socketserver
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('DAGGER_TEST_BIN', ROOT / 'target/release/dagger-rs')).resolve()


class SetupFailures(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.config = self.base / 'config'
        self.config.mkdir()
        self.binary = self.base / 'core'
        self.binary.write_text('#!/bin/sh\nexit 0\n')
        self.binary.chmod(0o755)
        self.env = dict(os.environ, DAGGER_BIN=str(self.binary), DAGGER_CONFIG_DIR=str(self.config))
        keys = self.config / 'server-keys'
        keys.mkdir()
        (keys / 'private.key').write_text('fixture')
        (keys / 'public.key').write_text('b' * 64)

    def menu(self, lines):
        return subprocess.run(['bash', str(ROOT / 'scripts/setup-classic.sh')],
                              input='\n'.join(lines) + '\n', env=self.env,
                              capture_output=True, text=True, timeout=10)

    def test_rejected_config_never_starts_old_config(self):
        old = self.config / 'server.json'
        old.write_text('{"original":true}')
        self.binary.write_text('#!/bin/sh\necho "core rejected config" >&2\nexit 1\n')
        result = self.menu(['1', 'a' * 64, 'yes', '1', 'ports', '0.0.0.0:7000',
                            'tcp', '0.0.0.0:18080', '127.0.0.1:8080', '', '0'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Configuration was rejected', result.stdout)
        self.assertNotIn('Step 2/2:', result.stdout)
        self.assertEqual(old.read_text(), '{"original":true}')
        self.assertEqual(list(self.config.glob('.config.*')), [])

    def test_cancel_replace_never_starts_old_config(self):
        old = self.config / 'server.json'
        old.write_text('{"original":true}')
        result = self.menu(['1', 'a' * 64, 'no', '', '0'])
        self.assertNotIn('Step 2/2:', result.stdout)
        self.assertIn('Setup cancelled', result.stdout)
        self.assertEqual(old.read_text(), '{"original":true}')

    def test_key_generation_failure_stops_action(self):
        self.binary.write_text('#!/bin/sh\necho "keygen failed" >&2\nexit 1\n')
        result = self.menu(['2', '', '0'])
        self.assertIn('Action failed', result.stdout)
        self.assertNotIn('Remote public key (', result.stdout)
        self.assertFalse((self.config / 'client.json').exists())

    def test_own_key_is_rejected_and_key_only_cancel_is_clear(self):
        result = self.menu(['1', 'b' * 64, '', '', '0'])
        self.assertIn('OTHER machine', result.stdout)
        self.assertIn('Setup cancelled', result.stdout)
        self.assertNotIn('Step 2/2:', result.stdout)

    def test_mid_config_eof_does_not_save_or_start(self):
        result = self.menu(['1', 'a' * 64, '1', 'ports'])
        self.assertIn('Input closed', result.stderr)
        self.assertNotIn('Step 2/2:', result.stdout)
        self.assertFalse((self.config / 'server.json').exists())
        self.assertEqual(list(self.config.glob('.config.*')), [])

    def test_action_stops_at_first_failed_command(self):
        marker = self.base / 'incorrect-success'
        script = 'source "$1"; broken() { false; touch "$2"; }; run_action broken unused "$2"; echo menu-alive'
        result = subprocess.run(['bash', '-c', script, 'test', str(ROOT / 'scripts/setup.sh'), str(marker)],
                                env=self.env, capture_output=True, text=True)
        self.assertFalse(marker.exists())
        self.assertIn('menu-alive', result.stdout)
        self.assertIn('Action failed', result.stdout)


    def test_service_restart_and_failures_are_reported(self):
        (self.config / 'server.json').write_text(json.dumps({
            'mode': 'server', 'listeners': [{'addr': '0.0.0.0:7000', 'maps': []}]}))
        shim = self.base / 'shim'
        shim.mkdir()
        trace = self.base / 'systemctl.trace'
        bodies = {'id': 'exit 0', 'chown': 'exit 0', 'runuser': 'exit 0', 'sleep': 'exit 0',
                  'journalctl': 'echo service-fixture-error',
                  'systemctl': """printf '%s\\n' "$*" >> "$SERVICE_TRACE"
for arg in "$@"; do
  if [ "$arg" = "$FAIL_ACTION" ]; then exit 1; fi
done
exit 0"""}
        for name, body in bodies.items():
            path = shim / name
            path.write_text('#!/bin/sh\n' + body + '\n')
            path.chmod(0o755)
        env = dict(self.env, PATH=str(shim) + os.pathsep + os.environ['PATH'],
                   SERVICE_TRACE=str(trace), TEST_UNITS=str(self.base / 'units'))
        script = 'source "$1"; service_unit_dir() { echo "$TEST_UNITS"; }; run_action install_service server'
        for failure in ('none', 'daemon-reload', 'enable', 'restart', 'is-active'):
            with self.subTest(failure=failure):
                trace.write_text('')
                result = subprocess.run(['bash', '-c', script, 'test', str(ROOT / 'scripts/setup.sh')],
                                        env=dict(env, FAIL_ACTION=failure), capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = trace.read_text()
                if failure == 'none':
                    self.assertIn('restart dagger-rs-server.service', calls)
                    self.assertIn('is-active --quiet dagger-rs-server.service', calls)
                    self.assertIn('Service running:', result.stdout)
                    unit = (self.base / 'units/dagger-rs-server.service').read_text()
                    self.assertIn('Environment=RUST_LOG=info,dagger_rs=debug', unit)
                else:
                    self.assertIn('Action failed', result.stdout)
                    self.assertNotIn('Service running:', result.stdout)
                    if failure in ('daemon-reload', 'enable'):
                        self.assertNotIn('restart ', calls)

    def test_missing_dial_address_cannot_be_accepted_as_localhost(self):
        keys = self.config / 'client-keys'
        keys.mkdir()
        (keys / 'private.key').write_text('fixture')
        (keys / 'public.key').write_text('c' * 64)
        result = self.menu(['2', 'a' * 64, '1', 'ports', '', 'bad:99999',
                            '192.0.2.1:7000', '127.0.0.1:8080', '1', 'no', '', '0'])
        config = json.loads((self.config / 'client.json').read_text())
        self.assertEqual(config['paths'][0]['addr'], '192.0.2.1:7000')
        self.assertEqual(result.stdout.count('Required: host:port'), 2)


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while True:
            data = self.request.recv(65536)
            if not data:
                return
            self.request.sendall(data)


class UdpEcho(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        sock.sendto(data, self.client_address)


def free_port(family=socket.AF_INET):
    with socket.socket(family) as sock:
        sock.bind(('::1' if family == socket.AF_INET6 else '127.0.0.1', 0))
        return sock.getsockname()[1]


@unittest.skipUnless(BINARY.is_file(), 'Build core or set DAGGER_TEST_BIN for real network tests')
class RealMenuForwarding(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.config = self.base / 'config'
        self.config.mkdir()
        self.env = dict(os.environ, DAGGER_BIN=str(BINARY), DAGGER_CONFIG_DIR=str(self.config))
        self.keys = {}
        for role in ('server', 'client'):
            path = self.config / (role + '-keys')
            subprocess.run([str(BINARY), 'keygen', '--out', str(path)], check=True, capture_output=True)
            self.keys[role] = (path / 'public.key').read_text().strip()
        self.cert = self.base / 'tls'
        subprocess.run([str(BINARY), 'certgen', '--out', str(self.cert), '--name', 'localhost'],
                       check=True, capture_output=True)
        self.processes = []
        self.logs = []
        self.addCleanup(self.stop_all)

    def stop_all(self):
        for process in self.processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in self.logs:
            log.close()

    def menu(self, lines):
        result = subprocess.run(['bash', str(ROOT / 'scripts/setup-classic.sh')],
                                input='\n'.join(lines + ['no', '', '0']) + '\n', env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('CONFIGURATION ONLY', result.stdout, result.stdout + result.stderr)
        self.assertNotIn('Action failed', result.stdout)

    def setup_pair(self, carrier, target, forwarded, kind='tcp', reverse=False):
        v6 = carrier == 'dc6'
        addr = f'[::1]:{free_port(socket.AF_INET6)}' if v6 else f'127.0.0.1:{free_port()}'
        numeric = dict(tcp='1', ws='2', wss='3', http='4', https='5',
                       kcp='13', xhttp='9', xhttps='10', dc6='11', **{'quantum+': '7'})
        for role in ('server', 'client'):
            opposite = 'client' if role == 'server' else 'server'
            lines = ['1' if role == 'server' else '2', self.keys[opposite]]
            if (self.config / (role + '.json')).exists():
                lines.append('yes')
            lines.append(numeric[carrier])
            if carrier in ('ws', 'wss', 'http', 'https', 'xhttp', 'xhttps'):
                lines.append('/tunnel')
            if carrier in ('xhttp', 'xhttps'):
                lines += ['yes' if reverse else 'no', 'packet-up' if reverse else 'stream-up']
                if (role == 'client') != reverse:
                    lines.append('localhost')
            if carrier in ('wss', 'https', 'xhttps'):
                if (role == 'server') != reverse:
                    lines += [str(self.cert / 'cert.pem'), str(self.cert / 'key.pem')]
                else:
                    if carrier != 'xhttps':
                        lines.append('localhost')
                    lines.append(str(self.cert / 'cert.pem'))
            lines += ['ports', addr]
            if role == 'server':
                if reverse:
                    lines.append('1')
                lines += [kind, forwarded]
                if kind != 'socks5':
                    lines.append(target)
            else:
                lines.append(target)
                if not reverse:
                    lines.append('1')
            self.menu(lines)

    def start(self, role):
        log = (self.base / f'{role}-{len(self.processes)}.log').open('w+')
        self.logs.append(log)
        process = subprocess.Popen([str(BINARY), '-c', str(self.config / (role + '.json'))],
                                   stdout=log, stderr=log,
                                   env=dict(os.environ, RUST_LOG='info,dagger_rs=debug'))
        self.processes.append(process)
        return process

    def echo_server(self, udp=False):
        cls = socketserver.ThreadingUDPServer if udp else socketserver.ThreadingTCPServer
        server = cls(('127.0.0.1', 0), UdpEcho if udp else Echo)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return f'127.0.0.1:{server.server_address[1]}'

    def exchange(self, forwarded, kind='tcp', target=None):
        host, port = forwarded.rsplit(':', 1)
        payload = os.urandom(4096 if kind == 'udp' else 100000)
        deadline = time.monotonic() + 12
        last_error = None
        while time.monotonic() < deadline:
            try:
                if kind == 'udp':
                    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
                        sock.settimeout(0.5)
                        sock.sendto(payload, (host, int(port)))
                        self.assertEqual(sock.recv(65536), payload)
                else:
                    with socket.create_connection((host, int(port)), timeout=1) as sock:
                        sock.settimeout(2)
                        if kind == 'socks5':
                            sock.sendall(b'\x05\x01\x00')
                            self.assertEqual(sock.recv(2), b'\x05\x00')
                            th, tp = target.rsplit(':', 1)
                            sock.sendall(b'\x05\x01\x00\x01' + socket.inet_aton(th) + int(tp).to_bytes(2, 'big'))
                            response = b''
                            while len(response) < 10:
                                data = sock.recv(10 - len(response))
                                if not data: raise OSError('SOCKS closed')
                                response += data
                            if response[1] != 0: raise OSError(f'SOCKS status {response[1]}')
                        received = b''
                        for offset in range(0, len(payload), 16384):
                            chunk = payload[offset:offset + 16384]
                            sock.sendall(chunk)
                            end = offset + len(chunk)
                            while len(received) < end:
                                data = sock.recv(end - len(received))
                                if not data: raise OSError('forwarded stream closed')
                                received += data
                        self.assertEqual(received, payload)
                return
            except OSError as error:
                last_error = error
                time.sleep(0.1)
        for log in self.logs:
            log.flush()
            log.seek(0)
            print(log.read()[-5000:])
        self.fail(f'No real forwarding: {kind}, {last_error}')

    def test_tcp_carrier_matrix(self):
        target = self.echo_server()
        for carrier in ('tcp', 'ws', 'http', 'wss', 'https', 'kcp', 'quantum+', 'xhttp', 'xhttps', 'dc6'):
            with self.subTest(carrier=carrier):
                forwarded = f'127.0.0.1:{free_port()}'
                self.setup_pair(carrier, target, forwarded)
                self.start('server')
                self.start('client')
                self.exchange(forwarded)
                self.stop_all()

    def test_udp_and_socks_forwarding(self):
        for kind in ('udp', 'socks5'):
            with self.subTest(kind=kind):
                target = self.echo_server(udp=kind == 'udp')
                forwarded = f'127.0.0.1:{free_port()}'
                self.setup_pair('tcp', target, forwarded, kind)
                self.start('server')
                self.start('client')
                self.exchange(forwarded, kind, target)
                self.stop_all()

    def test_reversed_xhttp_and_xhttps(self):
        target = self.echo_server()
        for carrier in ('xhttp', 'xhttps'):
            with self.subTest(carrier=carrier):
                forwarded = f'127.0.0.1:{free_port()}'
                self.setup_pair(carrier, target, forwarded, reverse=True)
                self.start('client')
                self.start('server')
                self.exchange(forwarded)
                self.stop_all()

    def test_recovers_after_peer_restart(self):
        target = self.echo_server()
        forwarded = f'127.0.0.1:{free_port()}'
        self.setup_pair('tcp', target, forwarded)
        self.start('server')
        client = self.start('client')
        self.exchange(forwarded)
        client.terminate()
        client.wait(timeout=3)
        self.start('client')
        self.exchange(forwarded)


if __name__ == '__main__':
    unittest.main()
