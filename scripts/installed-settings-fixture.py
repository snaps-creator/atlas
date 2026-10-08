"""Private synthetic data for disposable Windows acceptance, never a user profile."""
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import sqlite3
import sys
import time
import uuid

if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "Windows":
    raise SystemExit("Requires a disposable GitHub Windows runner")

class Blob(c.Structure):
    _fields_ = [("size", w.DWORD), ("data", c.c_void_p)]

class Credential(c.Structure):
    _fields_ = [("flags", w.DWORD), ("type", w.DWORD), ("target", w.LPWSTR),
                ("comment", w.LPWSTR), ("written", w.FILETIME), ("size", w.DWORD),
                ("blob", c.c_void_p), ("persist", w.DWORD), ("count", w.DWORD),
                ("attributes", c.c_void_p), ("alias", w.LPWSTR), ("user", w.LPWSTR)]

crypt = c.WinDLL("crypt32", use_last_error=True)
advapi = c.WinDLL("advapi32", use_last_error=True)
kernel = c.WinDLL("kernel32", use_last_error=True)
kernel.LocalFree.argtypes = [c.c_void_p]
advapi.CredFree.argtypes = [c.c_void_p]
advapi.CredReadW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, c.POINTER(c.POINTER(Credential))]
advapi.CredWriteW.argtypes = [c.POINTER(Credential), w.DWORD]

def dpapi(data, protect):
    buffer = c.create_string_buffer(data)
    source, target = Blob(len(data), c.cast(buffer, c.c_void_p)), Blob()
    fn = crypt.CryptProtectData if protect else crypt.CryptUnprotectData
    if not fn(c.byref(source), None, None, None, None, 1, c.byref(target)):
        raise c.WinError(c.get_last_error())
    try:
        return c.string_at(target.data, target.size)
    finally:
        kernel.LocalFree(target.data)

def credential(source, value=None):
    target = source + ".AtlasVPN"
    if value is not None:
        data = value.encode("utf-16-le")
        buffer = c.create_string_buffer(data)
        entry = Credential(type=1, target=target, size=len(data),
                           blob=c.cast(buffer, c.c_void_p), persist=2, user=source)
        if not advapi.CredWriteW(c.byref(entry), 0):
            raise c.WinError(c.get_last_error())
        return
    result = c.POINTER(Credential)()
    if not advapi.CredReadW(target, 1, 0, c.byref(result)):
        raise c.WinError(c.get_last_error())
    try:
        return c.string_at(result.contents.blob, result.contents.size).decode("utf-16-le")
    finally:
        advapi.CredFree(result)

directory = Path(os.environ["LOCALAPPDATA"]) / "net.atlasvpn.desktop"
directory.mkdir(exist_ok=True)
database = directory / "atlas.db"
action = sys.argv[1]
with sqlite3.connect(database, timeout=10) as db:
    if action == "seed":
        if db.execute("SELECT name FROM sqlite_master WHERE name='state'").fetchone():
            raise SystemExit("Refusing to replace existing application state")
        sources = []
        for kind in ["URL", "VLESS"]:
            identity = "acceptance-" + str(uuid.uuid4())
            client = str(uuid.uuid4())
            name = "Acceptance " + kind
            uri = f"vless://{client}@127.0.0.1:9?security=none&type=tcp#{kind}"
            value = "https://example.test/acceptance-only" if kind == "URL" else uri
            credential(identity, value)
            sources.append(dict(id=identity, name=name, source=kind,
                maskedUrl="https://example.test/***" if kind == "URL" else "vless://***",
                updatedAt=int(time.time()), error=None,
                options=dict(userAgentOverride="AtlasAcceptance/1", updateIntervalHours=24),
                servers=[dict(name=name, type="vless", server="127.0.0.1", port=9,
                              uuid=client, network="tcp", tls=False, udp=True)]))
        state = dict(subscriptions=sources, selected="Acceptance VLESS", favorites=["Acceptance URL", "Acceptance VLESS"],
            activeSource="VLESS", sourceSelections={"URL":"Acceptance URL", "VLESS":"Acceptance VLESS"},
            groups=[], defaultRoute="DIRECT", rulesSemanticsVersion=2, mode="tun", routingMode="direct", tunStack="gvisor",
            dns=dict(servers=["https://1.1.1.1/dns-query"], ipv6=False, fakeIp=True),
            startup=dict(launchWithWindows=True, autoConnect=False, startInTray=True, delaySeconds=1, restoreConnection=False),
            autoTestIntervalSeconds=300, autoSearchPingMs=150, theme="dark", wasConnected=False)
        db.executescript("CREATE TABLE state(id INTEGER PRIMARY KEY CHECK(id=1),payload BLOB NOT NULL);"
                         "CREATE TABLE backups(id INTEGER PRIMARY KEY AUTOINCREMENT,created INTEGER NOT NULL,payload BLOB NOT NULL);"
                         "PRAGMA user_version=2;")
        db.execute("INSERT INTO state VALUES(1,?)", (dpapi(json.dumps(state).encode(), True),))
        print("Synthetic encrypted URL + VLESS state and two user credentials seeded")
    else:
        state = json.loads(dpapi(db.execute("SELECT payload FROM state WHERE id=1").fetchone()[0], False))
        if action == "patch":
            patch = json.loads(sys.argv[2])
            state.update(patch)
            db.execute("UPDATE state SET payload=? WHERE id=1", (dpapi(json.dumps(state).encode(), True),))
        elif action == "verify":
            assert db.execute("PRAGMA user_version").fetchone()[0] == 3
            assert "activeSource" not in state and "sourceSelections" not in state
            assert len(state["subscriptions"]) == 2
            nodes = [n for source in state["subscriptions"] for n in source["servers"]]
            ids = {n["atlas"]["nodeId"] for n in nodes}
            assert len(ids) == 2 and set(state["favorites"]) == ids
            chosen = next(n for n in nodes if n["atlas"]["nodeId"] == state["selectedNodeId"])
            assert chosen["name"] == "Acceptance VLESS"
            for source in state["subscriptions"]:
                assert source["options"]["userAgentOverride"] == "AtlasAcceptance/1"
                assert source["options"]["updateIntervalHours"] == 24
                assert source["name"] == "Acceptance " + source["source"]
                value = credential(source["id"])
                if source["source"] == "VLESS":
                    assert source["servers"][0]["uuid"] in value and value.startswith("vless://")
                else:
                    assert value == "https://example.test/acceptance-only"
            print("PASS: schema, mixed sources, names, source IDs, selection, favorites, options and credentials")
        elif action == "state":
            print(json.dumps({k:state.get(k) for k in ["startup", "wasConnected", "userDisconnected", "lastWindowHidden"]}))
        else:
            raise SystemExit("Unknown action")
