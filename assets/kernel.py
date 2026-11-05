# Lightdown's Python helper. It runs notebook cells and answers completion
# and documentation requests, speaking JSON lines over the pipes it was started with. It is
# deliberately small: no Jupyter, no ZeroMQ, nothing to install.

import ast, asyncio, base64, builtins, importlib, importlib.util, inspect, io, json, keyword, linecache
import os, pkgutil, re, sys, textwrap, threading, traceback, time

_in = os.fdopen(os.dup(0), "rb")
_out = os.fdopen(os.dup(1), "wb")
_lock = threading.Lock()
_current = [None]


def send(**msg):
    data = (json.dumps(msg) + "\n").encode()
    with _lock:
        _out.write(data)
        _out.flush()


def _forward(fd, name):
    """Whatever subprocesses and C code write to the real stream is output too."""
    r, w = os.pipe()
    os.dup2(w, fd)
    os.close(w)

    def pump():
        while True:
            data = os.read(r, 65536)
            if not data:
                break
            send(type="stream", id=_current[0], name=name, text=data.decode("utf-8", "replace"))

    threading.Thread(target=pump, daemon=True).start()


os.dup2(os.open(os.devnull, os.O_RDONLY), 0)
_forward(1, "stdout")
_forward(2, "stderr")


class Stream(io.TextIOBase):
    def __init__(self, name, fd):
        self._name, self._fd = name, fd

    def writable(self):
        return True

    def write(self, s):
        if s:
            send(type="stream", id=_current[0], name=self._name, text=str(s))
        return len(s)

    def fileno(self):
        return self._fd


sys.stdin = open(os.devnull)
sys.stdout = Stream("stdout", 1)
sys.stderr = Stream("stderr", 2)

ns = {"__name__": "__main__", "__builtins__": builtins}
count = 0
_loop = None


_FORMS = (("_repr_png_", "image/png"), ("_repr_jpeg_", "image/jpeg"), ("_repr_svg_", "image/svg+xml"),
          ("_repr_html_", "text/html"), ("_repr_markdown_", "text/markdown"), ("_repr_latex_", "text/latex"),
          ("_repr_json_", "application/json"))


def show(obj, kind="display"):
    data = {}
    for method, mime in _FORMS:
        try:
            form = getattr(obj, method, None)
            value = form() if callable(form) else None
            if isinstance(value, tuple):
                value = value[0]
            if isinstance(value, bytes):
                value = base64.b64encode(value).decode()
            if value is not None and (not isinstance(value, str) or len(value) <= 4 * 1024 * 1024):
                data[mime] = value
        except Exception:
            pass
    try:
        data["text/plain"] = repr(obj)
    except Exception as e:
        data["text/plain"] = "<repr failed: %s>" % e
    send(type=kind, id=_current[0], data=data)


def display(*objs):
    for obj in objs:
        show(obj)


ns["display"] = display


def figures():
    """Open matplotlib figures are shown under the cell, as inline plots are."""
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return
    try:
        for num in plt.get_fignums():
            buf = io.BytesIO()
            plt.figure(num).savefig(buf, format="png", bbox_inches="tight")
            png = base64.b64encode(buf.getvalue()).decode()
            send(type="display", id=_current[0], png=png, text="<Figure>")
        plt.close("all")
    except Exception:
        traceback.print_exc()


def magics(code):
    """The few IPython lines notebooks commonly hold, as plain Python."""
    out = []
    for line in code.split("\n"):
        s = line.lstrip()
        pad = line[: len(line) - len(s)]
        if s.startswith("!"):
            out.append("%s__import__('subprocess').run(%r, shell=True)" % (pad, s[1:]))
        elif s.startswith("%pip "):
            out.append("%s__import__('subprocess').run([__import__('sys').executable, '-m', 'pip'] + %r)" % (pad, s[5:].split()))
        elif s.startswith("%matplotlib"):
            out.append(pad + "pass")
        else:
            out.append(line)
    return "\n".join(out)


def run(code, name, mode):
    global _loop
    obj = compile(code, name, mode, flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    value = eval(obj, ns)
    if obj.co_flags & inspect.CO_COROUTINE:
        _loop = _loop or asyncio.new_event_loop()
        value = _loop.run_until_complete(value)
    return value


def execute(msg):
    global count
    _current[0] = msg["id"]
    count += 1
    started = time.perf_counter()
    ran_at = time.time()
    ok = True
    name = "<cell %d>" % count
    try:
        code = magics(msg["code"])
        linecache.cache[name] = (len(code), None, code.splitlines(True), name)
        tree = ast.parse(code, name)
        last = None
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            last = ast.Expression(tree.body.pop().value)
        run(tree, name, "exec")
        if last is not None:
            value = run(last, name, "eval")
            if value is not None:
                ns["_"] = value
                show(value, "result")
    except KeyboardInterrupt:
        ok = False
        send(type="error", id=msg["id"], name="KeyboardInterrupt", value="", trace=["KeyboardInterrupt"])
    except BaseException as e:
        ok = False
        tb = e.__traceback__
        # Leave this file's own frames out of what the user sees.
        while tb is not None and not tb.tb_frame.f_code.co_filename.startswith("<cell"):
            tb = tb.tb_next
        lines = "".join(traceback.format_exception(type(e), e, tb)).rstrip("\n").split("\n")
        send(type="error", id=msg["id"], name=type(e).__name__, value=str(e), trace=lines)
    figures()
    send(type="timing", id=msg["id"], elapsed_ms=(time.perf_counter() - started) * 1000, ran_at=ran_at)
    variables()
    send(type="done", id=msg["id"], count=count, ok=ok)


def variables():
    items = []
    for name, obj in sorted(ns.copy().items()):
        if name.startswith("_") or name == "display" or inspect.ismodule(obj) or inspect.isroutine(obj) or inspect.isclass(obj):
            continue
        shape = ""
        try:
            dims = getattr(obj, "shape", None) if type(obj).__module__.split(".")[0] in ("numpy", "pandas", "torch", "xarray") else None
            if isinstance(dims, tuple):
                shape = " × ".join(str(int(n)) for n in dims[:16])
            elif isinstance(obj, (list, tuple, dict, set, str, bytes)):
                shape = "length %d" % len(obj)
        except BaseException:
            shape = "unavailable"
        items.append([name, type(obj).__name__, shape])
        if len(items) == 1000:
            break
    send(type="variables", items=items)


def kind_of(obj):
    if inspect.ismodule(obj):
        return "module"
    if inspect.isclass(obj):
        return "class"
    if callable(obj):
        return "function"
    return type(obj).__name__


def local_module(name, path):
    """Modules of the project itself are not imported just to complete names:
    importing runs their code."""
    try:
        spec = importlib.util.find_spec(name.split(".")[0])
    except Exception:
        return True
    origin = getattr(spec, "origin", None) if spec else None
    if spec is None:
        return True
    if not origin or origin in ("built-in", "frozen"):
        return False
    root = os.path.dirname(os.path.abspath(path)) if path else os.getcwd()
    return os.path.abspath(origin).startswith(root + os.sep) and "site-packages" not in origin


def imported(code, path):
    """What the file's own import statements bind, found line by line so
    that unfinished code elsewhere does not matter."""
    bound = {}
    for line in code.split("\n"):
        s = line.strip()
        if not (s.startswith("import ") or s.startswith("from ")):
            continue
        try:
            node = ast.parse(s).body[0]
        except SyntaxError:
            continue
        for alias in node.names:
            if isinstance(node, ast.Import):
                bound[alias.asname or alias.name.split(".")[0]] = (alias.name if alias.asname else alias.name.split(".")[0], None)
            elif node.module and not node.level:
                bound[alias.asname or alias.name] = (node.module, alias.name)
    return bound


def lookup(base, code, live, path):
    parts = base.split(".")
    if live and parts[0] in ns:
        obj = ns[parts[0]]
    elif hasattr(builtins, parts[0]):
        obj = getattr(builtins, parts[0])
    else:
        module, attr = imported(code, path).get(parts[0], (None, None))
        if module is None or local_module(module, path):
            return None
        obj = importlib.import_module(module)
        if attr is not None:
            obj = getattr(obj, attr, None) or importlib.import_module(module + "." + attr)
    for part in parts[1:]:
        obj = getattr(obj, part)
    return obj


def fallback(code, line, col, live, path):
    text = code.split("\n")[line - 1][:col]
    m = re.match(r"\s*from\s+([\w.]+)\s+import\s+(?:[\w, ]*,\s*)?(\w*)$", text)
    if m and not local_module(m.group(1), path):
        mod = importlib.import_module(m.group(1))
        names = set(dir(mod))
        if hasattr(mod, "__path__"):
            names.update(i.name for i in pkgutil.iter_modules(mod.__path__))
        return [[n, "name"] for n in sorted(names) if n.startswith(m.group(2)) and not n.startswith("_")]
    m = re.match(r"\s*(?:import|from)\s+([\w.]*)$", text)
    if m:
        pkg, _, prefix = m.group(1).rpartition(".")
        if not pkg:
            names = set(sys.builtin_module_names) | {i.name for i in pkgutil.iter_modules()}
        elif local_module(pkg, path):
            return []
        else:
            names = {i.name for i in pkgutil.iter_modules(importlib.import_module(pkg).__path__)}
        return [[n, "module"] for n in sorted(names) if n.startswith(prefix) and not n.startswith("_")]
    m = re.search(r"((?:[^\W\d]\w*\.)+)?([^\W\d]\w*)?$", text)
    base, prefix = (m.group(1) or "").rstrip("."), m.group(2) or ""
    if base:
        obj = lookup(base, code, live, path)
        if obj is None:
            return []
        names = [n for n in dir(obj) if n.startswith(prefix) and (prefix.startswith("_") or not n.startswith("_"))]
        items = []
        for n in names:
            try:
                items.append([n, kind_of(getattr(obj, n))])
            except Exception:
                items.append([n, ""])
        return items
    if not live or not prefix:
        return []
    return [[n, kind_of(v)] for n, v in sorted(ns.items()) if n.startswith(prefix) and not n.startswith("__")]


def complete(msg):
    code, line, col, live, path = msg["code"], msg["line"], msg["col"], msg.get("live"), msg.get("path")
    items = []
    try:
        try:
            import jedi

            script = jedi.Interpreter(code, [ns]) if live else jedi.Script(code, path=path)
            items = [[c.name, c.type] for c in script.complete(line, col)]
        except ImportError:
            items = fallback(code, line, col, live, path)
    except BaseException:
        pass
    send(type="matches", id=msg["id"], items=items[:60])


# ---- documentation ------------------------------------------------------

DOC_LIMIT = 8000


def doc_of(obj):
    try:
        doc = inspect.getdoc(obj) or ""
    except Exception:
        doc = ""
    return doc[:DOC_LIMIT]


def labelled(label, params):
    """The parameters with where each stands in the label, so that the one
    being typed can be picked out."""
    out, at = [], label.find("(") + 1
    for p in params:
        i = label.find(p, at)
        if i < 0:
            out.append([p, -1, -1])
        else:
            out.append([p, i, i + len(p)])
            at = i + len(p)
    return out


def described(name, obj):
    """What a live object says of itself."""
    info = {"name": name, "kind": kind_of(obj), "signature": "", "params": [], "doc": "", "module": ""}
    if inspect.ismodule(obj):
        info["signature"] = "module " + obj.__name__
        info["doc"] = doc_of(obj)
        return info
    if not (callable(obj) or inspect.isclass(obj)):
        # A value: its type is what there is to say.
        info["signature"] = "%s: %s" % (name, type(obj).__name__)
        if type(obj).__module__ not in ("builtins",):
            info["doc"] = doc_of(type(obj))
        return info
    module = getattr(obj, "__module__", None)
    if isinstance(module, str) and module != "builtins":
        info["module"] = module
    try:
        sig = inspect.signature(obj)
        params = [str(p) for p in sig.parameters.values()]
        prefix = "class " if inspect.isclass(obj) else ""
        label = prefix + name + str(sig)
        info["signature"], info["params"] = label, labelled(label, params)
    except (TypeError, ValueError):
        # Some functions written in C say how they are called in their
        # docstring's first line instead.
        first = (doc_of(obj).split("\n", 1) + [""])[0]
        if re.match(r"[\w.]+\(.*\)", first):
            info["signature"] = first
            inner = first[first.find("(") + 1 : first.rfind(")")]
            info["params"] = labelled(first, [p.strip() for p in inner.split(",") if p.strip()])
        else:
            info["signature"] = name
    info["doc"] = doc_of(obj)
    if inspect.isclass(obj) and not info["doc"]:
        info["doc"] = doc_of(getattr(obj, "__init__", None))
    return info


def block(code, kind, name):
    """The definition of a function or class by that name in the file,
    parsed on its own so that unfinished code elsewhere does not matter."""
    lines = code.split("\n")
    head = re.compile(r"(\s*)(?:async\s+)?%s\s+%s\b" % (kind, re.escape(name)))
    for i, line in enumerate(lines):
        m = head.match(line)
        if not m:
            continue
        depth, j = len(m.group(1)), i + 1
        while j < len(lines):
            s = lines[j]
            if s.strip() and len(s) - len(s.lstrip()) <= depth and not s.lstrip().startswith((")", "]", "}")):
                break
            j += 1
        # Decorators belong to it too.
        k = i
        while k > 0 and lines[k - 1].strip().startswith("@"):
            k -= 1
        try:
            node = ast.parse(textwrap.dedent("\n".join(lines[k:j]))).body[0]
        except (SyntaxError, IndexError):
            continue
        return node
    return None


def ast_params(args, method):
    """Each parameter of a definition as it is written."""
    def one(a, default=None, star=""):
        s = star + a.arg
        if a.annotation is not None:
            s += ": " + ast.unparse(a.annotation)
        if default is not None:
            s += (" = " if a.annotation is not None else "=") + ast.unparse(default)
        return s

    positional = args.posonlyargs + args.args
    defaults = [None] * (len(positional) - len(args.defaults)) + list(args.defaults)
    out = [one(a, d) for a, d in zip(positional, defaults)]
    if args.vararg:
        out.append(one(args.vararg, star="*"))
    for a, d in zip(args.kwonlyargs, args.kw_defaults):
        out.append(one(a, d))
    if args.kwarg:
        out.append(one(args.kwarg, star="**"))
    if method and positional and positional[0].arg in ("self", "cls"):
        out.pop(0)
    return out


def defined(name, code, method):
    """What a definition in the file itself says, without running it."""
    for kind in ("def", "class"):
        node = block(code, kind, name)
        if node is None:
            continue
        info = {"name": name, "kind": "function" if kind == "def" else "class", "module": "", "doc": ""}
        info["doc"] = (ast.get_docstring(node) or "")[:DOC_LIMIT]
        target = node
        if kind == "class":
            init = next((n for n in node.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name == "__init__"), None)
            target, method = init, True
            if init is not None and not info["doc"]:
                info["doc"] = (ast.get_docstring(init) or "")[:DOC_LIMIT]
        params = ast_params(target.args, method) if target is not None else []
        ret = ""
        if kind == "def" and node.returns is not None:
            ret = " -> " + ast.unparse(node.returns)
        label = "%s%s(%s)%s" % ("class " if kind == "class" else "", name, ", ".join(params), ret)
        info["signature"], info["params"] = label, labelled(label, params)
        return info
    return None


def jedi_info(code, line, col, live, path, call):
    import jedi

    script = jedi.Interpreter(code, [ns]) if live else jedi.Script(code, path=path)
    if call:
        sigs = script.get_signatures(line, col)
        if not sigs:
            return None
        sig = sigs[0]
        label = sig.to_string()
        info = {"name": sig.name, "kind": sig.type, "signature": label, "module": sig.module_name or ""}
        info["params"] = labelled(label, [p.to_string() for p in sig.params])
        info["doc"] = sig.docstring(raw=True)[:DOC_LIMIT]
        return info
    names = script.infer(line, col) or script.help(line, col)
    if not names:
        return None
    n = names[0]
    if n.type == "keyword":
        return None
    info = {"name": n.name, "kind": n.type, "module": n.module_name or "", "params": []}
    sigs = n.get_signatures() if n.type != "instance" else []
    if sigs:
        label = sigs[0].to_string()
        if n.type == "class":
            label = "class " + label
        info["signature"] = label
        info["params"] = labelled(label, [p.to_string() for p in sigs[0].params])
    elif n.type == "module":
        info["signature"] = "module " + (n.full_name or n.name)
    elif n.type == "instance":
        m = re.search(r"(\w+)$", code.split("\n")[line - 1][:col])
        info["signature"] = "%s: %s" % (m.group(1) if m else n.name, n.name)
        info["kind"] = "variable"
    else:
        info["signature"] = n.name
    info["doc"] = n.docstring(raw=True)[:DOC_LIMIT] if n.type != "instance" else ""
    return info


def plain_info(code, line, col, live, path):
    text = code.split("\n")[line - 1][:col]
    m = re.search(r"((?:[^\W\d]\w*\.)*[^\W\d]\w*)$", text)
    if not m:
        return None
    expr = m.group(1)
    parts = expr.split(".")
    if keyword.iskeyword(parts[-1]):
        return None
    try:
        obj = lookup(expr, code, live, path)
    except Exception:
        obj = None
    if obj is not None:
        return described(parts[-1], obj)
    if len(parts) == 1:
        return defined(parts[0], code, False)
    if len(parts) == 2 and parts[0] in ("self", "cls"):
        return defined(parts[1], code, True)
    return None


def info(msg):
    """The signature and documentation of the name ending at a place. For a
    call, `paren` is the column just inside its bracket, which jedi wants."""
    code, line, col, live, path = msg["code"], msg["line"], msg["col"], msg.get("live"), msg.get("path")
    paren = msg.get("paren")
    found = None
    try:
        try:
            found = jedi_info(code, line, col if paren is None else paren, live, path, paren is not None)
        except ImportError:
            found = plain_info(code, line, col, live, path)
        else:
            # jedi may know a name only by its typing stub, which has no
            # docstring; the object itself has one.
            if found and not found.get("doc"):
                plain = plain_info(code, line, col, live, path)
                if plain and plain.get("doc"):
                    found["doc"], found["module"] = plain["doc"], plain["module"] or found["module"]
    except BaseException:
        found = None
    if found and found.get("module") in ("builtins", "__main__"):
        found["module"] = ""
    if found and not (found.get("doc") or found.get("signature")):
        found = None
    send(type="info", id=msg["id"], info=found)


def main():
    sys.path.insert(0, os.getcwd())
    send(type="ready", version=sys.version.split()[0])
    while True:
        try:
            line = _in.readline()
            if not line:
                break
            msg = json.loads(line)
            if msg["op"] == "execute":
                execute(msg)
            elif msg["op"] == "complete":
                complete(msg)
            elif msg["op"] == "info":
                info(msg)
        except KeyboardInterrupt:
            pass
        except Exception:
            traceback.print_exc()


main()
