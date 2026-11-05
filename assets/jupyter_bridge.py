# Lightdown's bridge to Jupyter kernels. It starts one kernel with jupyter_client and
# speaks the same JSON lines as kernel.py over its own pipes, so the editor treats a
# Jupyter kernel like the built-in one. Started as: python -c <this> <config json>.

import json, os, queue, re, sys, threading, time

_out = os.fdopen(os.dup(1), "wb")
_lock = threading.Lock()


def send(**msg):
    data = (json.dumps(msg) + "\n").encode()
    with _lock:
        _out.write(data)
        _out.flush()


try:
    import zmq
    from jupyter_client.manager import KernelManager
    from jupyter_client.kernelspec import KernelSpec
except ImportError as e:
    send(type="missing", text="jupyter_client is not installed for %s (%s)" % (sys.executable, e))
    sys.exit(0)

config = json.loads(sys.argv[1])
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
NAME = re.compile(r"[\w.]*$")

VARS = (
    "[[k, type(v).__name__, (' × '.join(str(int(n)) for n in v.shape[:16]) "
    "if type(v).__module__.split('.')[0] in ('numpy', 'pandas', 'torch', 'xarray') "
    "and isinstance(getattr(v, 'shape', None), tuple) "
    "else ('length %d' % len(v) if isinstance(v, (list, tuple, dict, set, str, bytes)) else ''))] "
    "for k, v in sorted(list(globals().items())) if not k.startswith('_') "
    "and k not in ('In', 'Out', 'exit', 'quit', 'get_ipython', 'display') "
    "and not (__import__('inspect').ismodule(v) or __import__('inspect').isroutine(v) "
    "or __import__('inspect').isclass(v))][:1000]"
)


def launch():
    spec = config["spec"]
    fields = {k: spec[k] for k in ("argv", "display_name", "language", "interrupt_mode", "env", "metadata") if k in spec}
    km = KernelManager(kernel_name=config.get("name") or "")
    km._kernel_spec = KernelSpec(resource_dir=config.get("resource_dir") or "", **fields)
    km.start_kernel(cwd=config.get("cwd") or os.getcwd())
    kc = km.client()
    kc.allow_stdin = True
    kc.start_channels()
    try:
        kc.wait_for_ready(timeout=120)
    except RuntimeError as e:
        send(type="fatal", text="The kernel did not start: %s" % e)
        km.shutdown_kernel(now=True)
        sys.exit(0)
    return km, kc


def ops_reader(ops, wake_w):
    for line in sys.stdin:
        try:
            ops.put(json.loads(line))
        except ValueError:
            continue
        os.write(wake_w, b"x")
    ops.put({"op": "shutdown"})
    os.write(wake_w, b"x")


def cursor(code, line, col):
    lines = code.split("\n")
    return sum(len(l) + 1 for l in lines[: max(line - 1, 0)]) + col


def parse_inspect(text, name):
    """IPython's inspect text, split into the editor's fields; other kernels' text is the doc."""
    text = ANSI.sub("", text)
    fields, key, order = {}, None, []
    for line in text.split("\n"):
        m = re.match(r"^([A-Z][A-Za-z ]*):\s?(.*)$", line)
        if m and m.group(1) in ("Signature", "Init signature", "Call signature", "Docstring", "Init docstring",
                                "Type", "File", "String form", "Length", "Class docstring", "Source"):
            key = m.group(1)
            order.append(key)
            fields[key] = [m.group(2)] if m.group(2) else []
        elif key:
            fields[key].append(line)
        else:
            fields.setdefault("", []).append(line)
    get = lambda k: "\n".join(fields.get(k, [])).strip()
    sig = get("Signature") or get("Init signature") or get("Call signature")
    doc = get("Docstring") or get("Init docstring") or get("Class docstring")
    if doc == "<no docstring>":
        doc = ""
    kind = get("Type")
    if not order:
        doc = text.strip()
    if not sig and get("String form"):
        sig = "%s: %s" % (name, kind) if kind else name
    params = []
    if sig.endswith(")") and "(" in sig:
        start = sig.index("(") + 1
        depth, at = 0, start
        for i in range(start, len(sig)):
            c = sig[i]
            if c in "([{":
                depth += 1
            elif c in ")]}":
                if depth == 0:
                    if sig[at:i].strip():
                        params.append(sig[at:i])
                    break
                depth -= 1
            elif c == "," and depth == 0:
                params.append(sig[at:i])
                at = i + 1
        out, pos = [], start
        for p in params:
            stripped = p.strip()
            begin = sig.index(stripped, pos)
            out.append([stripped, begin, begin + len(stripped)])
            pos = begin + len(stripped)
        params = out
    module = get("File")
    return {"name": name, "kind": kind, "signature": sig or name, "params": params, "doc": doc,
            "module": os.path.basename(module) if module else ""}


def main():
    km, kc = launch()
    info = kc.kernel_info(reply=True, timeout=60)["content"]
    lang = info.get("language_info", {})
    python = lang.get("name") == "python" and info.get("implementation") == "ipython"
    send(type="ready", version=lang.get("version", ""), language_info=lang,
         implementation=info.get("implementation", ""), banner=info.get("banner", ""))

    ops = queue.Queue()
    wake_r, wake_w = os.pipe()
    threading.Thread(target=ops_reader, args=(ops, wake_w), daemon=True).start()
    poller = zmq.Poller()
    for ch in (kc.iopub_channel, kc.shell_channel, kc.stdin_channel):
        poller.register(ch.socket, zmq.POLLIN)
    poller.register(wake_r, zmq.POLLIN)

    pending = {}  # msg_id -> request
    current = [None]  # the execute request whose output is coming
    last_alive = time.time()

    def finish(req):
        if not (req["replied"] and req["idle"]):
            return
        pending.pop(req["msg_id"], None)
        send(type="timing", id=req["id"], elapsed_ms=(time.perf_counter() - req["started"]) * 1000, ran_at=req["ran_at"])
        if req.get("variables") is not None:
            send(type="variables", items=req["variables"])
        send(type="done", id=req["id"], count=req["count"], ok=req["ok"])
        if current[0] is req:
            current[0] = None

    while True:
        try:
            events = dict(poller.poll(1000))
        except KeyboardInterrupt:
            continue
        if wake_r in events:
            os.read(wake_r, 4096)
        while True:
            try:
                op = ops.get_nowait()
            except queue.Empty:
                break
            kind = op.get("op")
            if kind == "execute":
                expressions = {"__ld_vars": VARS} if python else {}
                msg_id = kc.execute(op["code"], allow_stdin=True, store_history=True, user_expressions=expressions)
                req = {"kind": "execute", "id": op["id"], "msg_id": msg_id, "replied": False, "idle": False,
                       "ok": True, "count": 0, "started": time.perf_counter(), "ran_at": time.time()}
                pending[msg_id] = req
                current[0] = current[0] or req
            elif kind == "complete":
                code = op["code"]
                pos = cursor(code, op["line"], op["col"])
                pending[kc.complete(code, pos)] = {"kind": "complete", "id": op["id"], "code": code, "pos": pos}
            elif kind == "info":
                code = op["code"]
                pos = cursor(code, op["line"], op["col"])
                head = code[:pos]
                if op.get("paren") is not None:
                    # Inside a call: ask about the name before the bracket.
                    line_start = head.rfind("\n") + 1
                    paren_at = line_start + op["paren"] - 1
                    if 0 <= paren_at < len(code):
                        pos = paren_at
                        head = code[:pos]
                name = NAME.search(head).group(0)
                pending[kc.inspect(code, pos, detail_level=0)] = {"kind": "info", "id": op["id"], "name": name}
            elif kind == "input":
                kc.input(op.get("value", ""))
            elif kind == "interrupt":
                km.interrupt_kernel()
            elif kind == "shutdown":
                kc.stop_channels()
                km.shutdown_kernel(now=False)
                return
        for _ in range(1000):
            try:
                msg = kc.get_iopub_msg(timeout=0)
            except queue.Empty:
                break
            t = msg["msg_type"]
            c = msg["content"]
            parent = msg["parent_header"].get("msg_id")
            req = pending.get(parent)
            if req is None or req["kind"] != "execute":
                continue
            rid = req["id"]
            current[0] = req
            if t == "stream":
                send(type="stream", id=rid, name=c.get("name", "stdout"), text=c.get("text", ""))
            elif t in ("display_data", "update_display_data", "execute_result"):
                display_id = (c.get("transient") or {}).get("display_id")
                send(type="result" if t == "execute_result" else "display", id=rid, data=c.get("data", {}),
                     metadata=c.get("metadata", {}), display_id=display_id, update=t == "update_display_data",
                     count=c.get("execution_count"))
            elif t == "error":
                req["ok"] = False
                send(type="error", id=rid, name=c.get("ename", ""), value=c.get("evalue", ""),
                     trace=c.get("traceback", []))
            elif t == "clear_output":
                send(type="clear", id=rid, wait=bool(c.get("wait")))
            elif t == "status" and c.get("execution_state") == "idle":
                req["idle"] = True
                finish(req)
        for _ in range(1000):
            try:
                msg = kc.get_shell_msg(timeout=0)
            except queue.Empty:
                break
            c = msg["content"]
            req = pending.get(msg["parent_header"].get("msg_id"))
            if req is None:
                continue
            if req["kind"] == "execute":
                req["replied"] = True
                req["count"] = c.get("execution_count") or 0
                if c.get("status") != "ok":
                    req["ok"] = False
                v = (c.get("user_expressions") or {}).get("__ld_vars")
                if v and v.get("status") == "ok":
                    try:
                        import ast
                        req["variables"] = ast.literal_eval(v["data"]["text/plain"])
                    except Exception:
                        pass
                finish(req)
            elif req["kind"] == "complete":
                pending.pop(msg["parent_header"]["msg_id"], None)
                start = c.get("cursor_start", req["pos"])
                ident = req["pos"] - len(re.search(r"\w*$", req["code"][: req["pos"]]).group(0))
                cut = req["code"][start:ident] if start < ident else ""
                types = {}
                for e in (c.get("metadata") or {}).get("_jupyter_types_experimental", []):
                    types[e.get("text")] = e.get("type", "")
                items = []
                for m in c.get("matches", []):
                    t = types.get(m, "")
                    if cut and m.startswith(cut):
                        m = m[len(cut):]
                    if m and [m, t] not in items:
                        items.append([m, t])
                send(type="matches", id=req["id"], items=items[:200])
            elif req["kind"] == "info":
                pending.pop(msg["parent_header"]["msg_id"], None)
                found = None
                if c.get("status") == "ok" and c.get("found"):
                    text = (c.get("data") or {}).get("text/plain", "")
                    found = parse_inspect(text, req["name"].rsplit(".", 1)[-1])
                send(type="info", id=req["id"], info=found)
        for _ in range(100):
            try:
                msg = kc.get_stdin_msg(timeout=0)
            except queue.Empty:
                break
            if msg["msg_type"] == "input_request":
                c = msg["content"]
                rid = current[0]["id"] if current[0] else 0
                send(type="input", id=rid, prompt=c.get("prompt", ""), password=bool(c.get("password")))
        if time.time() - last_alive > 1:
            last_alive = time.time()
            if not km.is_alive():
                send(type="fatal", text="The kernel died")
                kc.stop_channels()
                return


try:
    main()
except KeyboardInterrupt:
    pass
