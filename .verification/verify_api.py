#!/usr/bin/env python3
"""Compare public API surfaces between two ffs binaries + the C export list."""
import json, re, subprocess, sys

CMDS = ['find','glob','grep','multi-grep','read','outline','symbol','callers','callees',
        'refs','flow','siblings','deps','impact','index','map','overview','mcp','mention','guide']

def cli_surface(binp):
    def norm(t):
        return t.replace(binp.split('/')[-1], 'ffs')
    out = {}
    top = norm(subprocess.run([binp,'--help'],capture_output=True,text=True).stdout)
    out['__top__'] = (sorted(re.findall(r'(--[a-z][a-z0-9-]*)', top)), sorted(CMDS))
    for c in CMDS:
        t = norm(subprocess.run([binp,c,'--help'],capture_output=True,text=True).stdout)
        out[c] = (sorted(set(re.findall(r'(--[a-z][a-z0-9-]*)', t))),
                  sorted(set(re.findall(r'(?<![\w-])(-[a-zA-Z])(?=[, ])', t))),
                  next((l.strip() for l in t.splitlines() if l.lower().startswith('usage:')),''))
    return out

def mcp_surface(binp, root):
    p=subprocess.Popen([binp,'mcp',root],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
    def send(m,pr,rid):
        p.stdin.write((json.dumps({"jsonrpc":"2.0","id":rid,"method":m,"params":pr})+"\n").encode());p.stdin.flush()
        while True:
            l=p.stdout.readline()
            if not l: return {}
            try:
                r=json.loads(l)
                if r.get("id")==rid: return r
            except json.JSONDecodeError: pass
    send("initialize",{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"v","version":"1"}},0)
    p.stdin.write(json.dumps({"jsonrpc":"2.0","method":"notifications/initialized"}).encode()+b"\n");p.stdin.flush()
    tools=send("tools/list",{},2).get("result",{}).get("tools",[])
    p.terminate()
    return {t["name"]: (sorted(t["inputSchema"].get("properties",{})),
                        sorted(t["inputSchema"].get("required",[]))) for t in tools}

def c_exports(paths):
    out=[]
    for p in paths:
        out += re.findall(r'extern "C" fn (\w+)', open(p).read())
    return sorted(set(out))

before_bin, after_bin, fixture, repo = sys.argv[1:5]
cb, ca = cli_surface(before_bin), cli_surface(after_bin)
cli_diff = [c for c in cb if cb[c]!=ca.get(c)]
mb, ma = mcp_surface(before_bin, fixture), mcp_surface(after_bin, fixture)
mcp_missing = sorted(set(mb)-set(ma))
mcp_params_lost = {n: sorted(set(mb[n][0])-set(ma[n][0])) for n in mb if n in ma and set(mb[n][0])-set(ma[n][0])}
mcp_required_changed = {n:(mb[n][1],ma[n][1]) for n in mb if n in ma and mb[n][1]!=ma[n][1]}
c_b = c_exports([f'{repo}/.verification/c_before.rs', f'{repo}/.verification/engine_before.rs'])
c_a = c_exports([f'{repo}/crates/ffs-c/src/lib.rs', f'{repo}/crates/ffs-c/src/engine_ffi.rs'])

report = {
  "cli": {"subcommands": len(CMDS), "differences": cli_diff or "NONE"},
  "mcp_embedded": {"tool_count": len(mb), "removed": mcp_missing or "NONE",
                   "params_lost": mcp_params_lost or "NONE",
                   "required_changed": mcp_required_changed or "NONE"},
  "c_abi": {"count": len(c_a), "identical": c_b==c_a,
            "removed": [x for x in c_b if x not in c_a] or "NONE"},
}
ok = (not cli_diff and not mcp_missing and not mcp_params_lost
      and not mcp_required_changed and c_b==c_a)
report["verdict"] = "PASS" if ok else "FAIL"
print(json.dumps(report, indent=2))
sys.exit(0 if ok else 1)
