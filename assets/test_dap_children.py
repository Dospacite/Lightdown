"""A nested-target DAP fixture. Every child deliberately reuses local IDs."""
import json
import socket
import sys
import threading

source, port = sys.argv[1], int(sys.argv[2])
listener = socket.socket()
listener.bind(('127.0.0.1', port))
listener.listen()
links = {}
lock = threading.Lock()

def serve(sock):
    stream = sock.makefile('rb')
    name = 'parent'
    seq = 0
    def send(v):
        nonlocal seq
        with lock:
            seq += 1
            v['seq'] = seq
            body = json.dumps(v).encode()
            sock.sendall(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    def event(event, body={}):
        send({'type': 'event', 'event': event, 'body': body})
    while True:
        headers = {}
        while True:
            line = stream.readline()
            if not line:
                return
            if line == b'\r\n':
                break
            key, value = line.decode().split(':', 1)
            headers[key.lower()] = value.strip()
        v = json.loads(stream.read(int(headers['content-length'])))
        if v['type'] != 'request':
            continue
        c, a = v['command'], v.get('arguments', {})
        body = {}
        if c == 'initialize':
            body = {'supportsConfigurationDoneRequest': True,
                    'exceptionBreakpointFilters': [{'filter': 'all', 'default': True}]}
        elif c in ('launch', 'attach'):
            name = a.get('__pendingTargetId', 'parent')
            links[name] = send
        elif c == 'setBreakpoints':
            assert a['breakpoints'][0]['line'] == 2
            body = {'breakpoints': [{'verified': True}]}
            event('output', {'output': 'points-' + name + '\n'})
        elif c == 'setExceptionBreakpoints':
            assert a['filters'] == ['all']
            event('output', {'output': 'filters-' + name + '\n'})
        elif c == 'threads':
            body = {'threads': [] if name == 'parent' else [{'id': 0, 'name': name}]}
        elif c == 'stackTrace':
            assert a['threadId'] == 0
            body = {'stackFrames': [{'id': 0, 'name': name, 'line': 2, 'source': {'path': source}}]}
        elif c == 'scopes':
            assert a['frameId'] == 0
            body = {'scopes': [{'name': 'Locals', 'variablesReference': 1}]}
        elif c == 'variables':
            assert a['variablesReference'] in (1, 2)
            body = {'variables': [{'name': 'owner', 'value': name, 'variablesReference': 2 if a['variablesReference'] == 1 else 0}]}
        elif c == 'evaluate':
            assert a['frameId'] == 0
            body = {'result': 'owner-' + name}
        elif c == 'continue':
            assert a['threadId'] == 0
        send({'type': 'response', 'request_seq': v['seq'], 'command': c, 'success': True, 'body': body})
        if c == 'initialize':
            event('initialized')
        if c == 'configurationDone':
            if name == 'parent':
                send({'type': 'request', 'command': 'startDebugging', 'arguments': {'request': 'launch', 'configuration': {'type': 'pwa-node', '__pendingTargetId': 'one'}}})
            else:
                event('stopped', {'reason': 'breakpoint', 'threadId': 0})
        if c == 'continue':
            event('continued', {'threadId': 0})
            if name == 'one':
                send({'type': 'request', 'command': 'startDebugging', 'arguments': {'request': 'attach', 'configuration': {'type': 'pwa-node', '__pendingTargetId': 'two'}}})
                event('terminated')
                return
            else:
                links['parent']({'type': 'event', 'event': 'terminated', 'body': {}})
                return

while True:
    connection, _ = listener.accept()
    threading.Thread(target=serve, args=(connection,), daemon=True).start()
