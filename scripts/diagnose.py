#!/usr/bin/env python3
"""Explain actual saved dagger-rs routes; optionally probe explicit TCP endpoints.

This helper never edits configs, firewall rules or routes. TCP reachability is
not reported as proof of authentication or end-to-end forwarding.
"""
import argparse
import ipaddress
import json
from pathlib import Path
import socket


def endpoint(value):
    host, port = value.rsplit(':', 1)
    return host.strip('[]'), int(port)


def loopback(value):
    host, _ = endpoint(value)
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return host.lower() == 'localhost'


def tcp_probe(label, value):
    try:
        with socket.create_connection(endpoint(value), timeout=3):
            print(f'REACHABLE: {label} {value} accepts TCP. Authentication/data NOT verified.')
    except (OSError, ValueError) as error:
        print(f'UNREACHABLE: {label} {value}: {error}')


def report(config, summary_only=False):
    role = config['mode']
    reverse = config.get('reverse', False)
    print(f'\nSaved endpoint: {role}. Physical connection: '
          + ('server -> client/HTTP edge' if reverse else 'client -> server'))
    entries = config.get('listeners', []) + config.get('paths', [])
    for entry in entries:
        transport = entry.get('transport', 'tcp')
        dial = (role == 'client') != reverse
        print(f'  {transport}: {"connect to" if dial else "listen on"} {entry["addr"]}')
        if dial and loopback(entry['addr']) and not entry.get('raw'):
            print('  ATTENTION: loopback dials this machine, not the other server.')
        if not summary_only and dial and not entry.get('raw'):
            if transport not in ('kcp', 'quantum+'):
                tcp_probe('tunnel endpoint', entry['addr'])
            else:
                print('  UDP carrier: use the peer network tester and authenticated tunnel logs.')
        for mapping in entry.get('maps', []):
            print(f'  {mapping["type"].upper()} entry {mapping["bind"]} -> '
                  f'client service {mapping["target"]}')
            if loopback(mapping['bind']):
                print('  LOCAL ONLY: users on other machines cannot reach this forwarded bind.')
            print(f'  Client allowed_targets must include exactly: {mapping["target"]}')
    if config.get('socks5'):
        print(f'  SOCKS5 CONNECT: {config["socks5"]}; client must allow requested destinations.')
    if role == 'client':
        targets = config.get('allowed_targets', [])
        print('  Allowed destinations: ' + (', '.join(targets) or '(none)'))
        if not summary_only:
            for target in targets:
                if target != '*':
                    # The allowlist contains no protocol. Failure here may be normal
                    # for a UDP-only target; do not claim the tunnel itself failed.
                    tcp_probe('local target (TCP check; UDP-only services may not respond)', target)
    if config.get('tun'):
        tun = config['tun']
        print(f'  TUN {tun["name"]}: {tun["address"]} <-> {tun["peer_address"]}')
        print('  Routing/NAT beyond the peer interface requires explicit host configuration.')
    print('Next: configure BOTH peers, start both services, then test the real forwarded service.')
    print('A valid config or open TCP port alone does not prove an authenticated tunnel.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('config', type=Path)
    parser.add_argument('--summary-only', action='store_true')
    args = parser.parse_args()
    try:
        report(json.loads(args.config.read_text()), args.summary_only)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f'Cannot inspect configuration: {error}\n')
