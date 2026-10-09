"""Private synthetic data for disposable Windows acceptance, never a user profile."""
import ctypes as c
from ctypes import wintypes as w
import json
import hashlib
import ipaddress
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
credential_hashes = Path(os.environ["RUNNER_TEMP"]) / "atlas-acceptance-credential-hashes.json"
action = sys.argv[1]
with sqlite3.connect(database, timeout=10) as db:
    if action == "seed":
        port = int(sys.argv[2])
        address = str(ipaddress.IPv4Address(sys.argv[3]))
        assert 0 < port < 65536
        if db.execute("SELECT name FROM sqlite_master WHERE name='state'").fetchone():
            raise SystemExit("Refusing to replace existing application state")
        sources = []
        expected_credentials = {}
        clients = []
        for kind in ["URL", "VLESS"]:
            identity = "acceptance-" + str(uuid.uuid4())
            client = str(uuid.uuid4())
            clients.append(dict(id=client))
            name = "Acceptance " + kind
            uri = f"vless://{client}@{address}:{port}?security=none&type=tcp#{kind}"
            value = "https://example.test/acceptance-only" if kind == "URL" else uri
            credential(identity, value)
            expected_credentials[identity] = hashlib.sha256(value.encode()).hexdigest()
            sources.append(dict(id=identity, name=name, source=kind,
                maskedUrl="https://example.test/***" if kind == "URL" else "vless://***",
                updatedAt=int(time.time()), error=None,
                options=dict(userAgentOverride="AtlasAcceptance/1", updateIntervalHours=24),
                servers=[dict(name=name, type="vless", server=address, port=port,
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
        credential_hashes.write_text(json.dumps(expected_credentials), encoding="utf-8")
        server = dict(log=dict(loglevel="error"),
            inbounds=[dict(listen=address, port=port, protocol="vless",
                           settings=dict(clients=clients, decryption="none"),
                           streamSettings=dict(network="tcp", security="none"))],
            outbounds=[dict(protocol="freedom")])
        (Path(os.environ["RUNNER_TEMP"]) / "atlas-acceptance-vless.json").write_text(json.dumps(server), encoding="utf-8")
        print("Synthetic encrypted URL + VLESS state and two user credentials seeded")
    else:
        state = json.loads(dpapi(db.execute("SELECT payload FROM state WHERE id=1").fetchone()[0], False))
        if action == "prepare-upgrade":
            # The real 2.4.2 startup reparses VLESS and assigns its runtime name.
            # Establish user references AFTER that load, while 2.4.2 is stopped;
            # a seed-only name is not a valid installed user selection.
            assert db.execute("PRAGMA user_version").fetchone()[0] == 2
            assert {s["source"] for s in state["subscriptions"]} == {"URL", "VLESS"}
            nodes = [n for source in state["subscriptions"] for n in source["servers"]]
            assert len(nodes) == 2 and len({n["name"] for n in nodes}) == 2
            source = next(s for s in state["subscriptions"] if s["source"] == "VLESS")
            state["activeSource"] = "VLESS"
            state["selected"] = source["servers"][0]["name"]
            state["sourceSelections"] = {s["source"]:s["servers"][0]["name"] for s in state["subscriptions"]}
            state["favorites"] = [n["name"] for n in nodes]
            db.execute("UPDATE state SET payload=? WHERE id=1", (dpapi(json.dumps(state).encode(), True),))
            print("PASS: stopped real 2.4.2 has two valid favorites and an explicit VLESS selection before upgrade")
        elif action == "patch":
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
            vless = next(s for s in state["subscriptions"] if s["source"] == "VLESS")
            assert chosen["atlas"]["sourceId"] == vless["id"] and chosen["name"] == state["selected"]
            original = json.loads(dpapi(db.execute("SELECT payload FROM migration_backups3 WHERE version=3").fetchone()[0], False))
            assert original["selected"] == chosen["name"]
            assert set(original["favorites"]) == {n["name"] for n in nodes}
            assert {s["id"] for s in original["subscriptions"]} == {s["id"] for s in state["subscriptions"]}
            expected_credentials = json.loads(credential_hashes.read_text(encoding="utf-8"))
            assert set(expected_credentials) == {s["id"] for s in state["subscriptions"]}
            for source in state["subscriptions"]:
                assert source["options"]["userAgentOverride"] == "AtlasAcceptance/1"
                assert source["options"]["updateIntervalHours"] == 24
                assert source["name"] == "Acceptance " + source["source"]
                value = credential(source["id"])
                assert hashlib.sha256(value.encode()).hexdigest() == expected_credentials[source["id"]]
                if source["source"] == "VLESS":
                    node = source["servers"][0]
                    client = node.get("uuid") if node["type"] != "xray" else node["xray"]["outbounds"][0]["settings"]["vnext"][0]["users"][0]["id"]
                    assert client in value and value.startswith("vless://")
                else:
                    assert value == "https://example.test/acceptance-only"
            print("PASS: schema, mixed sources, names, source IDs, selection, favorites, options and credentials")
        elif action == "state":
            print(json.dumps({k:state.get(k) for k in ["startup", "wasConnected", "userDisconnected", "lastWindowHidden"]}))
        else:
            raise SystemExit("Unknown action")
