"""Offline daemon-headless contract regression: python3 daemon_headless.py BINARY."""
import json, os, pathlib, socket, subprocess, sys, tempfile, threading


def run(binary, failed, goal_only=False):
    with tempfile.TemporaryDirectory(prefix="headless-", dir="/tmp") as directory:
        root = pathlib.Path(directory); path = root / "daemon.sock"
        listener = socket.socket(socket.AF_UNIX); listener.bind(str(path)); listener.listen(); listener.settimeout(10)
        commands = []; errors = []
        message = {"role":"assistant","content":[{"type":"text","text":"owned daemon answer"}],
            "api":"faux","provider":"faux","model":"faux-1","stopReason":"error" if failed else "stop",
            "errorMessage":"fixture denied" if failed else None,"timestamp":1,
            "usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}}
        def serve():
            try:
                connection, _ = listener.accept()
                with connection:
                    connection.settimeout(10); stream = connection.makefile("rwb")
                    def send(value): stream.write((json.dumps(value)+"\n").encode()); stream.flush()
                    send({"type":"daemon_hello","protocol":{"name":"prime-agent.daemon","version":7}})
                    for line in stream:
                        request=json.loads(line); command=request["command"]; kind=command["type"]; commands.append(command)
                        data = None
                        if kind=="create": data={"activeSessionId":"root-active"}
                        elif kind=="get_session_header": data={"header":{"type":"session","id":"durable-root"}}
                        elif kind=="prompt_and_wait":
                            send({"type":"session_event","activeSessionId":"root-active","event":{"type":"message_end","message":message}})
                        elif kind=="get_messages": data={"messages":[{"role":"custom","customType":"rlm_child_terminal_notice","content":"child settled","display":True,"timestamp":"2026-10-13T17:31:20.444Z"},message]}
                        send({"type":"response","id":request["id"],"command":kind,"success":True,"data":data})
            except Exception as error: errors.append(repr(error))
        thread=threading.Thread(target=serve); thread.start()
        env={"PATH":os.environ["PATH"],"HOME":str(root),"DOTFILES_PRIME_NATIVE_SOCKET":str(root/"unused-policy"),
            "DOTFILES_PRIME_POLICY_SECRET":"fixture","PRIME_AGENT_FAUX_SCRIPT":json.dumps({"responses":["wrong local host"]})}
        try:
            result=subprocess.run([binary,"--print","--mode","json","--daemon-socket",str(path),"--no-tools",
                "--provider","faux","--model","faux-1","--models","faux/faux-1","--goal","fixture goal"] + ([] if goal_only else ["hello"]), env=env,cwd=root,text=True,capture_output=True,timeout=15)
            thread.join(11)
            assert not errors, errors
            assert result.returncode==(1 if failed else 0), (result.returncode,result.stderr)
            frames=[json.loads(line) for line in result.stdout.splitlines()]
            assert frames[0]=={"type":"session","id":"durable-root"}, frames
            if not goal_only: assert frames[1]["type"]=="message_end", frames
            create=commands[0]["config"]
            assert create["noTools"] is True and create["executionMode"]=="json", create
            assert create["initialGoal"]["objective"]=="fixture goal", create
            assert create["models"]==["faux/faux-1"], create
            expected = ["create","attach","get_system_prompt","get_session_header","get_messages"] if goal_only else ["create","attach","get_session_header","prompt_and_wait","wait_for_headless_completion","get_messages"]
            assert [c["type"] for c in commands]==expected, commands
            if not goal_only: assert commands[-2]["waitForRlmQuiescence"] is True
            print("daemon headless", "error" if failed else "success", "passed")
        finally:
            listener.close(); thread.join(11)

if __name__=="__main__":
    for failed in (False, True): run(sys.argv[1],failed)

    run(sys.argv[1],False,goal_only=True)

    with tempfile.TemporaryDirectory(prefix="headless-key-", dir="/tmp") as directory:
        env={"PATH":os.environ["PATH"],"HOME":directory,"DOTFILES_PRIME_NATIVE_SOCKET":directory+"/policy.sock","DOTFILES_PRIME_POLICY_SECRET":"fixture"}
        result=subprocess.run([sys.argv[1],"--print","--daemon-socket",directory+"/absent.sock","--api-key","fixture","hello"],env=env,cwd=directory,text=True,capture_output=True,timeout=10)
        assert result.returncode==1 and "--api-key is unsupported" in result.stderr, (result.returncode,result.stderr)
        print("daemon headless explicit API key rejected before connection")
