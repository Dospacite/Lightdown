"""Python debugger bridge. Only the standard library is required."""
import bdb
import contextlib
import io
import json
import os
import sys
import traceback
import itertools
import types

wire = sys.stdout
commands = sys.stdin

def emit(**event):
    wire.write(json.dumps(event, ensure_ascii=True) + "\n")
    wire.flush()

class Output(io.TextIOBase):
    def write(self, text):
        if text:
            emit(event="output", text=text[:16000])
        return len(text)
    def flush(self):
        pass

SIMPLE = (str, bytes, int, float, bool, complex, type(None))
TYPE_DICT = type.__dict__['__dict__']
TYPE_MRO = type.__dict__['__mro__']
TYPE_NAME = type.__dict__['__name__']
NAMED = {types.FunctionType: types.FunctionType.__dict__['__name__'],
         types.BuiltinFunctionType: types.BuiltinFunctionType.__dict__['__name__']}

def type_name(kind):
    try: return str(TYPE_NAME.__get__(kind))
    except BaseException: return "object"

def class_dicts(kind):
    """The namespaces of a class and its bases, read without its metaclass."""
    for cls in TYPE_MRO.__get__(kind):
        yield cls, TYPE_DICT.__get__(cls)

def builtin_container(value):
    # isinstance could ask the object for __class__; its type's MRO cannot.
    bases = TYPE_MRO.__get__(type(value))
    for base in (dict, list, tuple, set, frozenset):
        if base in bases: return base
    return None

def elements(value, base):
    if base is dict: return dict.items(value)
    return base.__iter__(value)

def describe(value, depth=0):
    """A short repr that never runs the program's own code."""
    kind = type(value)
    if kind in SIMPLE:
        if kind in (str, bytes): return repr(value[:200])[:500]
        if kind is int and int.bit_length(value) > 3000: return "<int of " + str(int.bit_length(value)) + " bits>"
        return repr(value)[:500]
    base = builtin_container(value)
    if base is not None:
        count = base.__len__(value)
        if depth >= 2 or kind is not base:
            return "<" + type_name(kind) + " of " + str(count) + ">"
        parts = []
        for item in itertools.islice(elements(value, base), 8):
            if base is dict: parts.append(describe(item[0], depth + 1) + ": " + describe(item[1], depth + 1))
            else: parts.append(describe(item, depth + 1))
        if count > 8: parts.append("…")
        inner = ", ".join(parts)
        if base is dict: text = "{" + inner + "}"
        elif base is tuple: text = "(" + inner + ("," if count == 1 else "") + ")"
        elif base is list: text = "[" + inner + "]"
        else: text = ("{" + inner + "}") if count else type_name(kind) + "()"
        return text[:500]
    if kind in NAMED:
        try: return "<function " + str(NAMED[kind].__get__(value)) + ">"
        except BaseException: pass
    if kind is type: return "<class " + type_name(value) + ">"
    if kind is types.ModuleType:
        name = module_dict(value).get("__name__")
        return "<module " + (name if type(name) is str else "?") + ">"
    return "<" + type_name(kind) + " object>"

def module_dict(module):
    namespace = types.ModuleType.__dict__['__dict__'].__get__(module)
    return namespace if type(namespace) is dict else {}

def attributes(value):
    """An object's own attributes, from its __dict__ and slots only."""
    kind = type(value)
    if kind is types.ModuleType:
        yield from ((k, v) for k, v in module_dict(value).items() if not (k.startswith("__") and k.endswith("__")))
        return
    if kind is type:
        yield from ((k, v) for k, v in TYPE_DICT.__get__(value).items() if not (k.startswith("__") and k.endswith("__")))
        return
    seen = set()
    for cls, namespace in class_dicts(kind):
        for name, descriptor in list(namespace.items()):
            if name in seen: continue
            if name == "__dict__" and type(descriptor) is types.GetSetDescriptorType:
                seen.add(name)
                try: own = descriptor.__get__(value, kind)
                except (AttributeError, TypeError): continue
                if type(own) is dict:
                    for key, item in dict.items(own):
                        if type(key) is str and key not in seen:
                            seen.add(key)
                            yield key, item
            elif type(descriptor) is types.MemberDescriptorType and name != "__weakref__":
                seen.add(name)
                try: yield name, descriptor.__get__(value, kind)
                except AttributeError: pass

def children(value):
    base = builtin_container(value)
    if base is dict:
        yield from ((describe(k), v) for k, v in dict.items(value))
    elif base is not None:
        yield from ((str(i), v) for i, v in enumerate(base.__iter__(value)))
    if type(value) not in SIMPLE and (base is None or type(value) is not base):
        yield from attributes(value)

def has_children(value):
    try:
        for _ in children(value): return True
    except BaseException: pass
    return False

MISSING = object()

def attribute(value, name):
    """One attribute, as stored: never a property, never __getattr__."""
    for key, item in attributes(value):
        if key == name: return item
    kind = value if type(value) is type else type(value)
    for cls, namespace in class_dicts(kind):
        if name in namespace:
            item = namespace[name]
            plain = type(item) in (types.FunctionType, staticmethod, classmethod)
            if not plain and any("__get__" in n for _, n in class_dicts(type(item))):
                raise LookupError(name + " is a property; it is not evaluated on hover")
            return item
    raise LookupError("no stored attribute " + name)

def resolve(expression, frame):
    """A name or dotted chain looked up without calling anything."""
    parts = expression.split(".")
    if not 0 < len(parts) <= 10 or len(expression) > 200 or not all(p.isidentifier() for p in parts):
        raise LookupError("only names and dotted chains are shown")
    first = parts[0]
    local = frame.f_locals
    if first in local: value = local[first]
    elif first in frame.f_globals: value = frame.f_globals[first]
    else:
        builtins = frame.f_globals.get("__builtins__", {})
        if type(builtins) is types.ModuleType: builtins = module_dict(builtins)
        if type(builtins) is not dict or first not in builtins: raise LookupError(first + " is not defined here")
        value = builtins[first]
    for name in parts[1:]:
        value = attribute(value, name)
    return value

def exception_text(exc):
    """An exception's message, when the standard library would make it."""
    kind = type(exc)
    for cls, namespace in class_dicts(kind):
        if "__str__" in namespace:
            builtin = type(namespace["__str__"]) is not types.FunctionType
            break
    else: builtin = True
    try: args = BaseException.__dict__['args'].__get__(exc)
    except BaseException: args = ()
    if builtin and type(args) is tuple and all(type(a) in SIMPLE for a in args):
        try: return str(exc)[:500]
        except BaseException: pass
    return ", ".join(describe(a) for a in args)[:500] if type(args) is tuple else ""

def project_file(filename):
    if not os.path.isabs(filename): return False
    inside = os.path.normcase(filename).startswith(os.path.normcase(project) + os.sep)
    return inside and not any(part in filename for part in (os.sep + "site-packages" + os.sep, os.sep + ".venv" + os.sep, os.sep + "venv" + os.sep))

class Debugger(bdb.Bdb):
    def __init__(self):
        super().__init__()
        self.frames = []
        self.selected = 0
        self.watches = []
        self.logpoints = {}
        self.references = {}
        self.raised = False
        self.uncaught = False
        self.last_raised = None
    def evaluate(self, expression, frame):
        # Watches and evaluations are asked for, so the program's repr may run.
        try:
            return repr(eval(expression, frame.f_globals, frame.f_locals))[:500]
        except BaseException as exc:
            return type(exc).__name__ + ": " + exception_text(exc)
    def set_points(self, points):
        self.clear_all_breaks()
        self.logpoints = {}
        for point in points:
            error = self.set_break(point["path"], point["line"], cond=point.get("condition") or None)
            if error: emit(event="output", text=error + "\n")
            if point.get("log"):
                self.logpoints[(self.canonic(point["path"]), point["line"])] = point["log"]
    def break_here(self, frame):
        hit = super().break_here(frame)
        expression = self.logpoints.get((self.canonic(frame.f_code.co_filename), frame.f_lineno))
        if hit and expression:
            emit(event="output", text="Logpoint: " + self.evaluate(expression, frame) + "\n")
            return False
        return hit
    def variable(self, name, value):
        reference = 0
        if type(value) not in SIMPLE and len(self.references) < 20000 and has_children(value):
            reference = len(self.references) + 1
            self.references[reference] = value
        return dict(name=str(name)[:500], type=type_name(type(value)), value=describe(value), variablesReference=reference)
    def snapshot(self):
        self.references = {}
        frame = self.frames[self.selected]
        emit(event="paused", reason=self.reason, exception=self.exception, selected=self.selected,
             stack=[dict(path=f.f_code.co_filename, line=f.f_lineno, name=f.f_code.co_name, start=f.f_code.co_firstlineno)
                    for f in self.frames],
             watches=[dict(expression=e, value=self.evaluate(e, frame)) for e in self.watches],
             expanded={}, variables=[self.variable(k, v)
                        for k, v in list(frame.f_locals.items())[:200]])
    def interaction(self, frame, reason, exception=None, frames=None):
        if frames is None:
            frames = []
            at = frame
            while at is not None:
                frames.append(at)
                if at is self.botframe:
                    break
                at = at.f_back
        self.frames, self.selected, self.reason, self.exception = frames, 0, reason, exception
        self.snapshot()
        for line in commands:
            try:
                cmd = json.loads(line)
                action = cmd.get("action")
                if action == "frame":
                    self.selected = max(0, min(len(self.frames) - 1, int(cmd["index"])))
                    self.snapshot()
                elif action == "breakpoints":
                    self.set_points(cmd["points"])
                elif action == "watches":
                    self.watches = cmd.get("watches", [])[:100]
                    self.snapshot()
                elif action == "expand":
                    reference = int(cmd["reference"])
                    start = min(20000, max(0, int(cmd.get("start", 0))))
                    page = [self.variable(k, v) for k, v in itertools.islice(children(self.references[reference]), start, start + 200)]
                    emit(event="children", reference=str(reference), start=start, variables=page)
                elif action == "hover":
                    expression = str(cmd.get("expression", ""))[:200]
                    try:
                        value = resolve(expression, self.frames[self.selected])
                        emit(event="hover", id=cmd.get("id"), expression=expression, type=type_name(type(value)), value=describe(value))
                    except BaseException as exc:
                        emit(event="hover", id=cmd.get("id"), expression=expression, error=str(exc)[:200])
                elif action == "exceptions":
                    self.raised = cmd.get("raised") is True
                    self.uncaught = cmd.get("uncaught") is True
                elif action == "evaluate":
                    selected = self.frames[self.selected]
                    emit(event="output", text=cmd.get("expression", "") + " = " + self.evaluate(cmd.get("expression", ""), selected) + "\n")
                elif action in ("continue", "next", "step", "out", "stop"):
                    if action == "continue": self.set_continue()
                    elif action == "next": self.set_next(frame)
                    elif action == "step": self.set_step()
                    elif action == "out": self.set_return(frame)
                    else: self.set_quit()
                    self.frames = []
                    self.references = {}
                    return
            except (ValueError, KeyError, TypeError, IndexError) as exc:
                emit(event="output", text=str(exc) + "\n")
        self.set_quit()
    def set_continue(self):
        # Raised exceptions are only seen while tracing goes on.
        if self.raised: self._set_stopinfo(self.botframe, None, -1)
        else: super().set_continue()
    def dispatch_call(self, frame, arg):
        result = super().dispatch_call(frame, arg)
        if result is None and self.raised and not self.quitting and project_file(frame.f_code.co_filename):
            return self.trace_dispatch
        return result
    def dispatch_exception(self, frame, arg):
        if self.stop_here(frame) or not self.raised:
            return super().dispatch_exception(frame, arg)
        # Each exception stops once, in the first project frame it reaches.
        kind, value = arg[0], arg[1]
        internal = kind in (StopIteration, StopAsyncIteration, GeneratorExit)
        if not internal and value is not self.last_raised and project_file(frame.f_code.co_filename):
            self.last_raised = value
            self.user_exception(frame, arg)
            if self.quitting: raise bdb.BdbQuit
        return self.trace_dispatch
    def user_line(self, frame):
        self.interaction(frame, "line")
    def user_return(self, frame, value):
        self.interaction(frame, "return " + describe(value))
    def user_exception(self, frame, exc):
        self.last_raised = exc[1]
        stopped = dict(type=type_name(exc[0]), message=exception_text(exc[1]))
        self.interaction(frame, "exception", stopped)

initial = json.loads(commands.readline())
path = os.path.abspath(initial["path"])
sys.argv = [path] + initial.get("args", [])
sys.path.insert(0, os.path.dirname(path))
sys.stdin = io.StringIO(initial.get("stdin", ""))
project = os.path.abspath(initial.get("root") or os.getcwd())
debugger = Debugger()
debugger.raised = initial.get("raised") is True
debugger.uncaught = initial.get("uncaught") is True
debugger.set_points(initial.get("points", []))
debugger.watches = initial.get("watches", [])[:100]
try:
    if initial.get("module") == "pytest":
        sys.argv = ["pytest"] + initial.get("args", [])
        code = compile("import runpy; runpy.run_module('pytest', run_name='__main__', alter_sys=True)", "<pytest>", "exec")
    else:
        with open(path, "rb") as source:
            code = compile(source.read(), path, "exec")
    scope = dict(__name__="__main__", __file__=path, __builtins__=__builtins__)
    with contextlib.redirect_stdout(Output()), contextlib.redirect_stderr(Output()):
        debugger.run(code, scope, scope)
except bdb.BdbQuit:
    pass
except BaseException as exc:
    if debugger.uncaught and SystemExit not in TYPE_MRO.__get__(type(exc)):
        # Post mortem: the frames are the traceback's, outside the bridge.
        frames, tb = [], exc.__traceback__
        while tb is not None:
            if tb.tb_frame.f_code.co_filename not in ("<string>", bdb.__file__): frames.insert(0, tb.tb_frame)
            tb = tb.tb_next
        if frames:
            debugger.quitting = False
            stopped = dict(type=type_name(type(exc)), message=exception_text(exc))
            debugger.interaction(frames[0], "uncaught exception", stopped, frames)
    emit(event="output", text=traceback.format_exc()[-20000:])
finally:
    emit(event="finished")
