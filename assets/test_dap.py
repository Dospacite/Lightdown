"""Test-only adapter fixture. Exercises DAP startup ordering on stdio and TCP."""
import json
import socket
import sys
source = sys.argv[1]
if len(sys.argv) > 2:
    server = socket.socket()
    server.bind(('127.0.0.1', int(sys.argv[2])))
    server.listen(1)
    connection, _ = server.accept()
    reader = connection.makefile('rb')
    writer = connection.makefile('wb')
else:
    reader, writer = sys.stdin.buffer, sys.stdout.buffer
seq = 0
launch = None

def send(value):
    global seq
    seq += 1
    value['seq'] = seq
    data = json.dumps(value).encode()
    writer.write(f'Content-Length: {len(data)}\r\n\r\n'.encode() + data)
    writer.flush()

def response(request, body=None):
    send(dict(type='response', request_seq=request['seq'], command=request['command'], success=True, body=body or {}))

def event(name, body=None):
    send(dict(type='event', event=name, body=body or {}))

while True:
    length = None
    while True:
        line = reader.readline()
        if not line:
            sys.exit(0)
        if line == b'\r\n':
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    request = json.loads(reader.read(length))
    command = request['command']
    if command == 'initialize':
        response(request, dict(supportsConfigurationDoneRequest=True, supportsExceptionInfoRequest=True,
            exceptionBreakpointFilters=[dict(filter='raised', label='Raised exceptions'),
                                        dict(filter='uncaught', label='Uncaught exceptions', default=True)]))
    elif command in ('launch', 'attach'):
        launch = request
        assert request['arguments']['program'] == source
        event('initialized')
    elif command == 'setBreakpoints':
        response(request, dict(breakpoints=[dict(verified=True)]))
    elif command == 'setExceptionBreakpoints':
        response(request)
        event('output', dict(output='filters ' + ','.join(request['arguments']['filters']) + '\n'))
    elif command == 'setFunctionBreakpoints':
        response(request)
    elif command == 'configurationDone':
        response(request)
        response(launch)
        event('stopped', dict(threadId=7, reason='breakpoint'))
    elif command == 'threads':
        response(request, dict(threads=[dict(id=7, name='main')]))
    elif command == 'stackTrace':
        assert request['arguments']['threadId'] == 7
        response(request, dict(stackFrames=[dict(id=10, name='main', source=dict(path=source), line=1)]))
    elif command == 'scopes':
        response(request, dict(scopes=[dict(name='Locals', expensive=False, variablesReference=20)]))
    elif command == 'variables':
        if request['arguments']['variablesReference'] == 30:
            response(request, dict(variables=[dict(name='child', type='int', value='3', variablesReference=0)]))
        else:
            response(request, dict(variables=[dict(name='answer', type='int', value='42', variablesReference=0), dict(name='object', type='Object', value='{...}', variablesReference=30)]))
    elif command == 'evaluate' and request['arguments'].get('context') == 'hover':
        response(request, dict(result='hover ' + request['arguments']['expression'], type='int'))
    elif command == 'evaluate':
        response(request, dict(result='evaluated 42'))
    elif command == 'exceptionInfo':
        response(request, dict(exceptionId='panic', description='index out of bounds', details=dict(typeName='core::panic')))
    elif command == 'stepIn':
        response(request)
        event('stopped', dict(threadId=7, reason='exception', text='panicked'))
    elif command in ('next', 'stepOut'):
        response(request)
        event('stopped', dict(threadId=7, reason='step'))
    elif command == 'continue':
        response(request)
        event('terminated')
        break
    else:
        raise AssertionError(command)
