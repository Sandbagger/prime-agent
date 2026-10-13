"""Offline RPC input lifecycle regression; usage: python3 rpc_native_inputs.py BINARY."""
import json, os, pathlib, queue, socket, subprocess, sys, tempfile, threading, time


def run(binary, lane):
    with tempfile.TemporaryDirectory(prefix="rpc-input-", dir="/tmp") as directory:
        root = pathlib.Path(directory)
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(root / "policy.sock")); listener.listen(); listener.settimeout(.1)
        stopped = threading.Event(); requests = []; failures = []
        def serve():
            while not stopped.is_set():
                try: connection, _ = listener.accept()
                except socket.timeout: continue
                with connection:
                    try:
                        connection.settimeout(3)
                        data = b""
                        while not data.endswith(b"\n"):
                            chunk = connection.recv(65536)
                            if not chunk: raise EOFError("incomplete policy request")
                            data += chunk
                        request = json.loads(data); requests.append(request)
                        assert request["secret"] == "offline-fixture"
                        reply = {"ok": True}
                        if request["op"] == "call_admit":
                            reply.update(permit_id=str(len(requests)), effort="high", service_tier="default")
                        connection.sendall((json.dumps(reply)+"\n").encode())
                    except Exception as error: failures.append(str(error))
        server = threading.Thread(target=serve); server.start()
        script = {"reasoning":True,"responses":["initial",{"delayMs":300,"text":"busy"},"queued","later"]}
        environment = {"PATH":os.environ["PATH"], "HOME":str(root), "PRIME_AGENT_CODING_AGENT_DIR":str(root/"agent"),
            "PRIME_AGENT_FAUX_SCRIPT":json.dumps(script), "DOTFILES_PRIME_NATIVE_SOCKET":str(root/"policy.sock"),
            "DOTFILES_PRIME_POLICY_SECRET":"offline-fixture"}
        process = subprocess.Popen([binary,"--mode","rpc","--provider","faux","--model","faux-1","--thinking","high"],
            cwd=root,env=environment,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        frames = queue.Queue()
        def read():
            for line in process.stdout: frames.put(json.loads(line))
        reader = threading.Thread(target=read); reader.start()
        def send(kind, message):
            process.stdin.write(json.dumps({"type":kind,"message":message})+"\n"); process.stdin.flush()
        def wait(kind):
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                frame=frames.get(timeout=max(.01,deadline-time.monotonic()))
                if frame.get("type")==kind:return frame
            raise TimeoutError(kind)
        def settle():
            wait("agent_end")
            # AgentEnd is delivered just before the agent releases its run slot.
            deadline=time.monotonic()+3
            while time.monotonic()<deadline:
                process.stdin.write('{"type":"get_state","id":"state"}\n'); process.stdin.flush()
                frame=wait("response")
                if frame.get("id")=="state" and not frame.get("data",{}).get("isStreaming",True):return
            raise TimeoutError("idle")
        try:
            send(lane,"initial idle"); settle()
            send("prompt","busy root"); wait("agent_start")
            send(lane,"busy queued input"); settle()
            send(lane,"later idle"); settle()
            process.stdin.close(); process.wait(timeout=10); reader.join(timeout=2)
            deadline=time.monotonic()+2
            while time.monotonic()<deadline and not any(r["op"]=="telemetry_close" for r in requests):time.sleep(.01)
            begins=[r for r in requests if r["op"]=="telemetry_begin"]
            assert [r["prompt"] for r in begins]==["initial idle","busy root","later idle"], begins
            assert sum(r["op"]=="root_rearm" for r in requests)==2, requests
            assert not failures, failures
            assert process.returncode==0, process.stderr.read()
            print(lane+": initial idle, subsequent idle, and busy continuation passed")
        finally:
            if process.poll() is None:process.kill();process.wait()
            reader.join(timeout=2); stopped.set(); server.join(timeout=2); listener.close()

if __name__=="__main__":
    for lane in ("follow_up","steer"):run(sys.argv[1],lane)
