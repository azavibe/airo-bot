#!/usr/bin/env python3
"""Small, dependency-free client used by the Air agent skill."""

from __future__ import annotations

import argparse
import difflib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen
import uuid


DEFAULT_BASE_URL = "http://localhost:8080"
POLL_INTERVAL_SECONDS = 2


class AirError(RuntimeError):
    pass


def config_dir() -> Path:
    override = os.environ.get("AIR_CONFIG_DIR")
    if override:
        return Path(override).expanduser()
    if sys.platform == "win32":
        root = os.environ.get("APPDATA")
        return Path(root) / "Air" if root else Path.home() / "AppData" / "Roaming" / "Air"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "Air"
    root = os.environ.get("XDG_CONFIG_HOME")
    return (Path(root) if root else Path.home() / ".config") / "air"


def workspace_dir() -> Path:
    override = os.environ.get("AIR_WORKSPACE_DIR")
    return Path(override).expanduser() if override else Path.cwd() / ".air"


def secure_directory(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    try:
        path.chmod(stat.S_IRWXU)
    except OSError:
        pass


def write_secure_json(path: Path, value: dict[str, Any]) -> None:
    secure_directory(path.parent)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        try:
            temporary.chmod(stat.S_IRUSR | stat.S_IWUSR)
        except OSError:
            pass
        temporary.replace(path)
        try:
            path.chmod(stat.S_IRUSR | stat.S_IWUSR)
        except OSError:
            pass
    finally:
        if temporary.exists():
            temporary.unlink()


def read_json(path: Path, *, required: bool = False) -> dict[str, Any]:
    if not path.exists():
        if required:
            raise AirError(f"missing local file: {path}")
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AirError(f"could not read {path}: {error}") from error
    if not isinstance(value, dict):
        raise AirError(f"expected a JSON object in {path}")
    return value


def config_path() -> Path:
    return config_dir() / "config.json"


def state_path() -> Path:
    return workspace_dir() / "state.json"


def draft_path() -> Path:
    return workspace_dir() / "procedure.json"


def load_config() -> dict[str, Any]:
    config = read_json(config_path(), required=True)
    if not config.get("base_url") or not config.get("agent_key"):
        raise AirError("Air login is incomplete; run /air login")
    return config


def api_request(
    base_url: str,
    method: str,
    path: str,
    payload: dict[str, Any] | None = None,
    agent_key: str | None = None,
    timeout: int = 40,
) -> Any:
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    headers = {"Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    if agent_key:
        headers["Authorization"] = f"Bearer {agent_key}"
    request = Request(
        f"{base_url.rstrip('/')}{path}", data=data, headers=headers, method=method
    )
    try:
        with urlopen(request, timeout=timeout) as response:
            raw = response.read()
    except HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        try:
            detail = json.loads(detail).get("error", detail)
        except json.JSONDecodeError:
            pass
        raise AirError(f"Air returned HTTP {error.code}: {detail}") from error
    except URLError as error:
        raise AirError(f"could not reach Air: {error.reason}") from error
    if not raw:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError as error:
        raise AirError("Air returned an invalid JSON response") from error


def authenticated_request(
    method: str, path: str, payload: dict[str, Any] | None = None, timeout: int = 40
) -> Any:
    config = load_config()
    return api_request(
        str(config["base_url"]),
        method,
        path,
        payload,
        str(config["agent_key"]),
        timeout,
    )


def login(args: argparse.Namespace) -> None:
    base_url = args.base_url.rstrip("/")
    response = api_request(base_url, "POST", "/device/code", {})
    code = str(response["code"])
    device_code = str(response["device_code"])
    print(f"Open: {response['verification_url']}")
    print(f"Code: {code}")
    print("Waiting for approval...", flush=True)

    deadline = time.monotonic() + int(response.get("expires_in", 600))
    while time.monotonic() < deadline:
        result = api_request(
            base_url, "POST", "/device/token", {"device_code": device_code}
        )
        status_value = result.get("status")
        if status_value == "approved":
            write_secure_json(
                config_path(),
                {
                    "base_url": base_url,
                    "agent_key": result["agent_key"],
                    "agent_name": result.get("agent_name"),
                },
            )
            print(f"Connected as {result.get('agent_name') or 'agent'}.")
            return
        if status_value in {"denied", "expired"}:
            raise AirError(f"device login was {status_value}")
        time.sleep(POLL_INTERVAL_SECONDS)
    raise AirError("device login expired")


def note(args: argparse.Namespace) -> None:
    text = args.text.strip()
    if not text:
        raise AirError("note text must not be empty")
    state = read_json(state_path())
    thread_id = state.get("thread_id")
    if not thread_id:
        title = args.title or Path.cwd().name or "agent task"
        payload: dict[str, Any] = {
            "title": title,
            "visibility": "private",
            "idempotency_key": f"skill-thread-{uuid.uuid4()}",
        }
        workspace = getattr(args, "workspace", None) or state.get("workspace_name")
        if workspace:
            payload["board_name"] = workspace
        thread = authenticated_request("POST", "/threads", payload)
        thread_id = thread["id"]
        state["thread_id"] = thread_id
        state["thread_title"] = thread["title"]
        state.setdefault("cursor", 0)
        write_secure_json(state_path(), state)

    result = authenticated_request(
        "POST",
        f"/threads/{thread_id}/notes",
        {
            "body": text,
            "kind": "status",
            "idempotency_key": args.idempotency_key or f"skill-note-{uuid.uuid4()}",
        },
    )
    print(f"Note written to thread {result['thread_id']}.")


def agents_list(_args: argparse.Namespace) -> None:
    result = authenticated_request("GET", "/agents")
    agents = result.get("agents", [])
    if not agents:
        print("No active connected agents.")
        return
    print(f"Connected agents ({len(agents)}):")
    for a in agents:
        last_seen = a.get("last_seen_at") or "never"
        status = "offline" if a.get("quiet") else "active"
        print(f"  - {a['name']} (ID: {a['id']}, status: {status}, last seen: {last_seen})")


def workspace_list(_args: argparse.Namespace) -> None:
    result = authenticated_request("GET", "/boards")
    boards = result.get("boards", [])
    if not boards:
        print("No workspaces found.")
        return
    print("Workspaces:")
    for b in boards:
        print(f"  {b['name']}: {b.get('workflow_count', 0)} workflows, {b.get('agent_count', 0)} agents, {b.get('thread_count', 0)} tasks ({b.get('description', '')})")


def workspace_create(args: argparse.Namespace) -> None:
    payload: dict[str, Any] = {
        "name": args.name.strip().lstrip("/"),
        "description": args.description or "",
        "visibility": args.visibility,
    }
    result = authenticated_request("POST", "/boards", payload)
    board = result.get("board", result)
    print(f"Created workspace '{board.get('name')}' (ID: {board.get('id')}).")


def workspace_edit(args: argparse.Namespace) -> None:
    name = args.name.strip().lstrip("/")
    payload: dict[str, Any] = {}
    if args.description is not None:
        payload["description"] = args.description
    if args.visibility is not None:
        payload["visibility"] = args.visibility
    result = authenticated_request("PATCH", f"/boards/{name}", payload)
    board = result.get("board", result)
    print(f"Updated workspace '{board.get('name')}'.")


def workspace_show(args: argparse.Namespace) -> None:
    name = args.name.strip().lstrip("/")
    result = authenticated_request("GET", f"/boards/{name}")
    board = result.get("board", {})
    workflows = result.get("workflows", [])
    runs = result.get("runs", [])
    threads = result.get("threads", [])
    agents = result.get("assigned_agents", [])
    print(f"Workspace: {board.get('name')} [{board.get('visibility')}]")
    if board.get("description"):
        print(f"Description: {board.get('description')}")
    print(f"\nWorkflows ({len(workflows)}):")
    for wf in workflows:
        print(f"  - {wf['title']} (ID: {wf['id']}, steps: {wf.get('step_count', 0)}, runs: {wf.get('run_count', 0)})")
    print(f"\nRuns ({len(runs)}):")
    for r in runs:
        outcome_str = f" / {r.get('outcome')}" if r.get("outcome") else ""
        print(f"  - Run {r['id']} [{r['status']}{outcome_str}] (step {r['current_step_order']} of {r['total_steps']})")
    print(f"\nAssigned agents ({len(agents)}):")
    for a in agents:
        print(f"  - {a['name']} ({a['role']}, ID: {a['agent_id']})")
    print(f"\nTasks ({len(threads)}):")
    for t in threads:
        print(f"  - [{t['status']}] {t['title']} (ID: {t['id']})")


def workspace_assign(args: argparse.Namespace) -> None:
    name = args.name.strip().lstrip("/")
    payload = {
        "agent_id": args.agent_id.strip(),
        "role": args.role or "bot",
    }
    result = authenticated_request("POST", f"/boards/{name}/agents", payload)
    print(f"Assigned agent {payload['agent_id']} to workspace '{name}' as {payload['role']}.")


def workspace_select(args: argparse.Namespace) -> None:
    name = args.name.strip().lstrip("/")
    result = authenticated_request("GET", f"/boards/{name}")
    board = result.get("board", {})
    state = read_json(state_path())
    state["workspace_name"] = board.get("name", name)
    state["workspace_id"] = board.get("id")
    write_secure_json(state_path(), state)
    print(f"Active workspace set to '{state['workspace_name']}' ({state['workspace_id']}).")


def workspace_status(_args: argparse.Namespace) -> None:
    state = read_json(state_path())
    workspace_name = state.get("workspace_name")
    if not workspace_name:
        print("No active workspace selected. Use 'workspace select <name>'.")
        return
    result = authenticated_request("GET", f"/boards/{workspace_name}")
    board = result.get("board", {})
    workflows = result.get("workflows", [])
    threads = result.get("threads", [])
    agents = result.get("assigned_agents", [])
    print(f"Workspace: {board.get('name')} [{board.get('visibility')}]")
    print(f"Description: {board.get('description')}")
    print(f"\nWorkflows ({len(workflows)}):")
    for wf in workflows:
        print(f"  - {wf['title']} (ID: {wf['id']}, steps: {wf.get('step_count', 0)})")
    print(f"\nAssigned agents ({len(agents)}):")
    if not agents:
        print("  (none)")
    else:
        for a in agents:
            print(f"  - {a['name']} ({a['role']})")
    print(f"\nTasks / Threads ({len(threads)}):")
    if not threads:
        print("  (none)")
    else:
        for t in threads:
            print(f"  - [{t['status']}] {t['title']} ({t['note_count']} notes, id: {t['id']})")


def workflow_list(args: argparse.Namespace) -> None:
    path = "/workflows"
    if args.workspace:
        path += f"?board={args.workspace.strip().lstrip('/')}"
    result = authenticated_request("GET", path)
    workflows = result.get("workflows", [])
    if not workflows:
        print("No workflows found.")
        return
    print(f"Workflows ({len(workflows)}):")
    for wf in workflows:
        board_str = f" in {wf['board_name']}" if wf.get("board_name") else ""
        print(f"  - {wf['title']}{board_str} (ID: {wf['id']}, steps: {wf.get('step_count', 0)}, v{wf.get('current_version', 1)})")


def workflow_show(args: argparse.Namespace) -> None:
    wf_id = args.id.strip()
    result = authenticated_request("GET", f"/workflows/{wf_id}")
    wf = result.get("workflow", {})
    steps = result.get("steps", [])
    runs = result.get("runs", [])
    print(f"Workflow: {wf.get('title')} (v{wf.get('current_version')}) [{wf.get('visibility')}]")
    print(f"Summary: {wf.get('summary')}")
    print(f"Instructions: {wf.get('body')}")
    print(f"Completion check: {wf.get('check_description')}")
    print(f"\nOrdered steps ({len(steps)}):")
    for s in steps:
        agent_str = f" [Agent: {s.get('agent_name') or s.get('agent_id')}]" if s.get("agent_id") else " [Unassigned]"
        print(f"  {s['step_order']}. {s['title']}{agent_str}")
        print(f"     Instructions: {s['instructions']}")
    if runs:
        print(f"\nRecent runs ({len(runs)}):")
        for r in runs:
            outcome_str = f" / {r.get('outcome')}" if r.get("outcome") else ""
            print(f"  - Run {r['id']} [{r['status']}{outcome_str}]")


def workflow_create(args: argparse.Namespace) -> None:
    body_text = ""
    if args.body_file:
        body_text = Path(args.body_file).read_text(encoding="utf-8").strip()
    elif args.instructions:
        body_text = args.instructions.strip()
    else:
        raise AirError("provide either --body-file or --instructions")

    steps: list[dict[str, Any]] = []
    if args.steps_file:
        steps_raw = json.loads(Path(args.steps_file).read_text(encoding="utf-8"))
        if isinstance(steps_raw, list):
            steps = steps_raw
    elif args.step:
        for idx, item in enumerate(args.step, start=1):
            parts = item.split(":", 2)
            step_dict: dict[str, Any] = {"step_order": idx, "title": parts[0].strip()}
            if len(parts) > 1:
                step_dict["instructions"] = parts[1].strip()
            else:
                step_dict["instructions"] = parts[0].strip()
            if len(parts) > 2 and parts[2].strip():
                step_dict["agent_id"] = parts[2].strip()
            steps.append(step_dict)

    payload: dict[str, Any] = {
        "title": args.title.strip(),
        "summary": args.summary.strip(),
        "body": body_text,
        "check_description": args.check.strip(),
        "visibility": args.visibility,
        "board": args.workspace.strip().lstrip("/") if args.workspace else None,
        "steps": steps,
        "idempotency_key": args.idempotency_key or f"workflow-create-{uuid.uuid4()}",
    }
    result = authenticated_request("POST", "/workflows", payload)
    wf = result.get("workflow", result)
    print(f"Created workflow '{wf.get('title')}' (ID: {wf.get('id')}) with {len(result.get('steps', steps))} steps.")


def workflow_edit(args: argparse.Namespace) -> None:
    wf_id = args.id.strip()
    payload: dict[str, Any] = {}
    if args.title:
        payload["title"] = args.title.strip()
    if args.summary:
        payload["summary"] = args.summary.strip()
    if args.body_file:
        payload["body"] = Path(args.body_file).read_text(encoding="utf-8").strip()
    if args.check:
        payload["check_description"] = args.check.strip()
    if args.visibility:
        payload["visibility"] = args.visibility
    if args.steps_file:
        payload["steps"] = json.loads(Path(args.steps_file).read_text(encoding="utf-8"))
    result = authenticated_request("PATCH", f"/workflows/{wf_id}", payload)
    wf = result.get("workflow", result)
    print(f"Updated workflow '{wf.get('title')}' (ID: {wf.get('id')}).")


def run_start(args: argparse.Namespace) -> None:
    wf_id = args.workflow_id.strip()
    payload: dict[str, Any] = {
        "idempotency_key": args.idempotency_key or f"run-start-{uuid.uuid4()}",
    }
    if args.model:
        payload["model"] = args.model
    result = authenticated_request("POST", f"/workflows/{wf_id}/runs", payload)
    run = result.get("run", result)
    print(f"Started run {run.get('id')} for workflow {wf_id}. Status: {run.get('status')}. Thread: {run.get('thread_id')}")


def run_show(args: argparse.Namespace) -> None:
    run_id = args.run_id.strip()
    result = authenticated_request("GET", f"/runs/{run_id}")
    run = result.get("run", {})
    steps = result.get("steps", [])
    outcome_str = f" / {run.get('outcome')}" if run.get("outcome") else ""
    print(f"Run {run.get('id')} [{run.get('status')}{outcome_str}]")
    print(f"Workflow: {run.get('workflow_title')} (v{run.get('procedure_version')})")
    print(f"Completion check: {run.get('workflow_check')}")
    if run.get("thread_id"):
        print(f"Activity thread: {run.get('thread_id')}")
    print(f"\nSteps ({len(steps)}):")
    for s in steps:
        agent_info = f" [Agent: {s.get('agent_name') or s.get('agent_id')}]" if s.get("agent_id") else ""
        print(f"  Step {s['step_order']}: {s['title']} [{s['status']}]{agent_info}")
        if s.get("input"):
            inp_preview = s['input'][:100].replace('\n', ' ')
            print(f"    Input: {inp_preview}...")
        if s.get("output"):
            out_preview = s['output'][:100].replace('\n', ' ')
            print(f"    Output: {out_preview}...")


def step_current(_args: argparse.Namespace) -> None:
    result = authenticated_request("GET", "/agent/steps/current")
    step = result.get("step")
    if not step:
        print("No pending step assigned.")
        return
    print(f"Assigned step for run {step['run_id']} (step {step['step_order']}):")
    print(f"  Workflow: {step['workflow_title']}")
    print(f"  Title: {step['title']}")
    print(f"  Instructions:\n{step['instructions']}")
    if step.get("input"):
        print(f"\nPrior step output (Input):\n{step['input']}")
    print(f"\nTo submit your result, run:")
    print(f"  air step complete {step['run_id']} {step['step_order']} --result \"<result_text>\"")


def step_complete(args: argparse.Namespace) -> None:
    run_id = args.run_id.strip()
    step_order = args.step_order
    output_text = ""
    if args.result_file:
        output_text = Path(args.result_file).read_text(encoding="utf-8").strip()
    elif args.result:
        output_text = args.result.strip()
    else:
        raise AirError("provide either --result or --result-file")
    payload = {
        "output": output_text,
        "outcome": args.outcome or "pass",
        "idempotency_key": args.idempotency_key or f"step-complete-{uuid.uuid4()}",
    }
    result = authenticated_request("POST", f"/runs/{run_id}/steps/{step_order}/result", payload)
    print(f"Submitted step {step_order} result for run {run_id}. Run status: {result.get('status')}")


def clean_runner_output(output: str) -> str:
    if not output:
        return ""
    lines = output.splitlines()
    cleaned_lines = []
    skip_arrow_block = False
    for line in lines:
        stripped = line.strip()
        if not stripped:
            if not skip_arrow_block:
                cleaned_lines.append(line)
            continue
        # Tool call headers like [agentgrep], [read], [bash], [batch], [Tokens...]
        if re.match(r"^\[[a-zA-Z0-9_.-]+(?::[^\]]*)?\]", stripped):
            skip_arrow_block = True
            continue
        # Arrow result lines like "→ query: ..." or "→ 1 ---"
        if stripped.startswith("→"):
            skip_arrow_block = True
            continue
        # Command echo lines
        if stripped.startswith("$ ") or stripped.startswith("usage: "):
            continue
        skip_arrow_block = False
        cleaned_lines.append(line)

    result = "\n".join(cleaned_lines).strip()
    return result or output.strip()


def run_jcode_headless(prompt: str, cwd: str) -> str:
    jcode_bin = "/root/.local/bin/jcode" if Path("/root/.local/bin/jcode").exists() else "jcode"
    cmd = [jcode_bin, "-C", cwd, "run", "--quiet", "--json", prompt]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode == 0 and proc.stdout.strip():
        try:
            data = json.loads(proc.stdout.strip())
            text = str(data.get("text", "")).strip()
            if text:
                return clean_runner_output(text)
        except (json.JSONDecodeError, KeyError, TypeError):
            pass
    raw = proc.stdout.strip()
    return clean_runner_output(raw)


def run_agy_headless(prompt: str, cwd: str, effort: str = "low") -> str:
    agy_bin = "/root/.local/bin/agy" if Path("/root/.local/bin/agy").exists() else "agy"
    # Headless print mode (-p) with disabled slash commands and direct text stdout
    cmd = [
        agy_bin,
        "--effort",
        effort,
        "--disable-slash-commands",
        "--dangerously-skip-permissions",
        "-p",
        prompt,
    ]
    try:
        proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=60)
        if proc.returncode == 0 and proc.stdout.strip():
            text = proc.stdout.strip()
            if text.startswith("{") and text.endswith("}"):
                try:
                    data = json.loads(text)
                    extracted = str(data.get("response", data.get("text", ""))).strip()
                    if extracted:
                        return clean_runner_output(extracted)
                except (json.JSONDecodeError, KeyError, TypeError):
                    pass
            return clean_runner_output(text)
        raw = proc.stdout.strip()
        return clean_runner_output(raw)
    except Exception as err:
        print(f"⚠️ AGY headless (-p) execution failed: {err}")
        return ""


def run_opencode_headless(prompt: str, cwd: str) -> str:
    opencode_bin = (
        "/root/.opencode/bin/opencode"
        if Path("/root/.opencode/bin/opencode").exists()
        else "opencode"
    )
    # `run --format json` streams one JSON event per line; the assistant text is
    # collected from the "text" parts instead of scraping the decorated TUI
    # stdout. --auto keeps a dispatched step from stalling on a permission
    # prompt the daemon cannot answer.
    cmd = [opencode_bin, "run", "--auto", "--format", "json", prompt]
    try:
        proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    except Exception as err:
        print(f"⚠️ OpenCode headless execution failed: {err}")
        return ""
    pieces: list[str] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "text":
            text = str(event.get("part", {}).get("text", "")).strip()
            if text:
                pieces.append(text)
    if pieces:
        return clean_runner_output("\n".join(pieces))
    return clean_runner_output(proc.stdout.strip())


def execute_headless_prompt(prompt: str, cwd: str, runner: str = "auto", effort: str = "low") -> str:
    runner_key = (runner or "auto").lower()
    if runner_key == "agy":
        reply = run_agy_headless(prompt, cwd, effort=effort)
        if not reply:
            print("⚠️ AGY runner returned empty/failed; trying Jcode fallback...")
            reply = run_jcode_headless(prompt, cwd)
        return reply
    elif runner_key == "jcode":
        reply = run_jcode_headless(prompt, cwd)
        if not reply:
            print("⚠️ Jcode runner returned empty/failed; trying AGY fallback...")
            reply = run_agy_headless(prompt, cwd, effort=effort)
        return reply
    elif runner_key == "opencode":
        reply = run_opencode_headless(prompt, cwd)
        if not reply:
            print("⚠️ OpenCode runner returned empty/failed; trying AGY fallback...")
            reply = run_agy_headless(prompt, cwd, effort=effort)
        return reply
    else:  # auto
        if Path("/root/.local/bin/agy").exists():
            reply = run_agy_headless(prompt, cwd, effort=effort)
            if not reply:
                print("⚠️ AGY runner returned empty/failed; trying Jcode fallback...")
                reply = run_jcode_headless(prompt, cwd)
            return reply
        else:
            reply = run_jcode_headless(prompt, cwd)
            if not reply:
                reply = run_agy_headless(prompt, cwd, effort=effort)
            return reply


def execute_step_with_jcode(step: dict[str, Any], workspace: str | None = None) -> str:
    return execute_step(step, workspace, "jcode")


def execute_step(step: dict[str, Any], workspace: str | None = None, runner: str = "auto") -> str:
    order = step.get("step_order")
    title = step.get("title", "")
    instructions = step.get("instructions", "")
    input_text = step.get("input", "")

    prompt = (
        f"You are executing an assigned workflow step for Air.\n\n"
        f"Task Title: {title}\n"
        f"Instructions:\n{instructions}\n\n"
    )
    if input_text:
        prompt += f"Input from previous steps:\n{input_text}\n\n"
    prompt += "Please execute the instructions and output your final result/solution clearly."

    cwd = workspace or str(Path("/root/harness/projects/air") if Path("/root/harness/projects/air").exists() else Path.cwd())
    print(f"Executing step {order} ('{title}') via headless runner ({runner})...")
    output = execute_headless_prompt(prompt, cwd, runner)
    return output or "(No output produced by runner)"


def daemon_run(args: argparse.Namespace) -> None:
    interval = getattr(args, "interval", 30) or 30
    runner = getattr(args, "runner", "auto") or "auto"
    workspace = getattr(args, "workspace", None)
    config = load_config()
    base_url = str(config.get("base_url", "https://airo-bot.com"))
    agent_name = str(config.get("agent_name", "agent"))
    agent_key = str(config.get("agent_key", ""))
    current_agent_id = ""
    if agent_key.startswith("air_ag_"):
        current_agent_id = agent_key[len("air_ag_") :].split(".")[0]

    print(f"Air daemon started for '{agent_name}' ({base_url}).")
    print(f"Heartbeat interval: {interval}s. Runner: {runner}.")
    if runner in ("jcode", "agy", "auto"):
        print(f"⚡ Autonomous mode: Steps and chat messages will be executed headless via {runner.upper()} and submitted to Airboard.")
    else:
        print("Polling /agent/steps/current and /inbox to stay online. Press Ctrl+C to stop.")

    last_executed_step_key = None
    last_heartbeat_time = 0.0
    state = read_json(state_path())
    cursor = int(state.get("cursor", 0))

    while True:
        now_str = time.strftime("%Y-%m-%d %H:%M:%S")
        inbox_had_error = False

        # 1. Process incoming chat commands and thread notes from /inbox (Instant Long-Poll)
        try:
            inbox_res = authenticated_request("GET", f"/inbox?{urlencode({'since': cursor})}")
            notes = inbox_res.get("notes", []) if isinstance(inbox_res, dict) else []
            new_cursor = int(inbox_res.get("cursor", cursor)) if isinstance(inbox_res, dict) else cursor
            if new_cursor > cursor:
                cursor = new_cursor
                state["cursor"] = cursor
                write_secure_json(state_path(), state)

            for item in notes:
                author_agent = item.get("author_agent_id")
                # Respond if the note was written by human owner or another party (not this agent)
                if author_agent != current_agent_id:
                    thread_id = item.get("thread_id")
                    body = item.get("body", "").strip()
                    if not body or not thread_id:
                        continue
                    print(f"[{now_str}] 📩 Note received in thread {thread_id}: {body}")
                    if runner in ("jcode", "agy", "auto"):
                        print(f"[{now_str}] Launching headless runner ({runner}) to respond to user...")
                        try:
                            prompt = (
                                f"You are agent '{agent_name}' on the Air noticeboard (https://airo-bot.com).\n"
                                f"Your human owner sent you this live chat message in thread {thread_id}:\n\n"
                                f"\"{body}\"\n\n"
                                f"INSTRUCTIONS:\n"
                                f"- Answer your human owner directly, helpfully, and concisely.\n"
                                f"- For questions, conversation, status updates, or general reasoning, answer immediately without making tool calls or reading files.\n"
                                f"- Only use tools if the user explicitly commanded an action (like running a command or editing a specific file)."
                            )
                            cwd = workspace or str(Path("/root/harness/projects/air") if Path("/root/harness/projects/air").exists() else Path.cwd())
                            reply_text = execute_headless_prompt(prompt, cwd, runner, effort="low")
                            if not reply_text:
                                reply_text = f"Acknowledged by {agent_name}."
                            payload = {
                                "body": reply_text,
                                "kind": "status",
                                "idempotency_key": f"daemon-chat-{uuid.uuid4()}",
                            }
                            authenticated_request("POST", f"/threads/{thread_id}/notes", payload)
                            print(f"[{now_str}] ✅ Replied to thread {thread_id}: {reply_text[:80]}...")
                        except Exception as reply_err:
                            print(f"[{now_str}] ❌ Error replying to thread with {runner}: {reply_err}")
                    else:
                        print(f"[{now_str}] (Runner is '{runner}'; chat note logged without automated reply)")
        except Exception as inbox_err:
            inbox_had_error = True
            print(f"[{now_str}] ⚠️ Inbox check failed: {inbox_err}")

        # 2. Process assigned workflow steps & heartbeat on configured interval
        now_ts = time.time()
        if now_ts - last_heartbeat_time >= interval:
            last_heartbeat_time = now_ts
            try:
                result = authenticated_request("GET", "/agent/steps/current")
                step = result.get("step") if isinstance(result, dict) else None
                if step:
                    step_key = f"{step.get('run_id')}_{step.get('step_order')}"
                    print(
                        f"[{now_str}] 🟢 Heartbeat OK. Pending step assigned: "
                        f"Run {step.get('run_id')} Step {step.get('step_order')} ('{step.get('title')}')"
                    )
                    if runner in ("jcode", "agy", "auto"):
                        if step_key == last_executed_step_key:
                            print(f"[{now_str}] Step {step_key} already processed. Waiting for next step.")
                        else:
                            print(f"[{now_str}] Launching headless runner ({runner}) to execute step...")
                            try:
                                output_text = execute_step(step, workspace, runner)
                                print(f"[{now_str}] Headless runner completed step. Submitting result to Airboard...")
                                payload = {
                                    "output": output_text,
                                    "outcome": "pass",
                                    "idempotency_key": f"step-daemon-{uuid.uuid4()}",
                                }
                                authenticated_request(
                                    "POST",
                                    f"/runs/{step['run_id']}/steps/{step['step_order']}/result",
                                    payload,
                                )
                                last_executed_step_key = step_key
                                print(f"[{now_str}] ✅ Step {step.get('step_order')} submitted successfully!")
                            except Exception as step_err:
                                print(f"[{now_str}] ❌ Error running step with {runner}: {step_err}")
                else:
                    print(f"[{now_str}] 🟢 Heartbeat OK. Online 24/7 on Air ({agent_name}). No pending steps.")
            except Exception as err:
                print(f"[{now_str}] ⚠️ Heartbeat failed: {err}")

        # Sleep briefly to yield CPU, or back off longer on network failure
        try:
            if inbox_had_error:
                time.sleep(3)
            else:
                time.sleep(0.1)
        except KeyboardInterrupt:
            print("\nAir daemon stopped.")
            break


def daemon_install(args: argparse.Namespace) -> None:
    python_bin = sys.executable or "/usr/bin/python3"
    script_path = Path(__file__).resolve()
    work_dir = script_path.parents[3]
    interval = getattr(args, "interval", 30) or 30
    runner = getattr(args, "runner", "auto") or "auto"
    agent_name = load_config().get("agent_name", "agent")
    safe_name = re.sub(r"[^a-zA-Z0-9_-]", "-", agent_name.lower()).strip("-") or "agent"
    service_name = getattr(args, "service_name", None) or f"air-agent-{safe_name}"
    config_dir_env = os.environ.get("AIR_CONFIG_DIR", "")

    env_line = f"Environment=\"AIR_CONFIG_DIR={config_dir_env}\"\n" if config_dir_env else ""
    unit_content = f"""[Unit]
Description=Air Noticeboard 24/7 Agent Daemon ({agent_name})
After=network.target

[Service]
Type=simple
WorkingDirectory={work_dir}
{env_line}ExecStart={python_bin} {script_path} daemon run --interval {interval} --runner {runner}
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
"""
    user_systemd_dir = Path.home() / ".config" / "systemd" / "user"
    service_file_name = f"{service_name}.service"
    installed_path = None
    try:
        user_systemd_dir.mkdir(parents=True, exist_ok=True)
        service_file = user_systemd_dir / service_file_name
        service_file.write_text(unit_content, encoding="utf-8")
        installed_path = service_file
        print(f"Wrote systemd user unit to {installed_path}")
    except OSError as err:
        fallback_file = work_dir / service_file_name
        fallback_file.write_text(unit_content, encoding="utf-8")
        print(f"Notice: Could not write directly to {user_systemd_dir} ({err}).")
        print(f"Wrote systemd unit file to {fallback_file}")
        installed_path = fallback_file

    try:
        (work_dir / service_file_name).write_text(unit_content, encoding="utf-8")
    except OSError:
        pass

    try:
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=True, capture_output=True)
        subprocess.run(["systemctl", "--user", "enable", "--now", service_file_name], check=True, capture_output=True)
        print(f"Successfully enabled and started {service_file_name} via systemctl --user.")
    except Exception:
        print("\nTo activate and start the service with systemctl (outside restricted sandbox):")
        print("  mkdir -p ~/.config/systemd/user")
        print(f"  cp {installed_path} ~/.config/systemd/user/{service_file_name}")
        print("  systemctl --user daemon-reload")
        print(f"  systemctl --user enable --now {service_file_name}")
        print(f"  systemctl --user status {service_file_name}")


def daemon_status(args: argparse.Namespace) -> None:
    agent_name = load_config().get("agent_name", "agent")
    safe_name = re.sub(r"[^a-zA-Z0-9_-]", "-", agent_name.lower()).strip("-") or "agent"
    service_name = getattr(args, "service_name", None) or f"air-agent-{safe_name}"
    service_file_name = f"{service_name}.service"
    user_service = Path.home() / ".config" / "systemd" / "user" / service_file_name
    workspace_service = Path(__file__).resolve().parents[3] / service_file_name
    if user_service.exists():
        print(f"Service unit found at: {user_service}")
    elif workspace_service.exists():
        print(f"Service unit found at: {workspace_service}")
    else:
        print(f"No {service_file_name} unit file found. Run 'daemon install' first.")

    try:
        res = subprocess.run(["systemctl", "--user", "status", service_file_name], text=True, capture_output=True)
        print(res.stdout or res.stderr)
    except Exception as err:
        print(f"systemctl --user status query could not be executed directly: {err}")


def workspace_task(args: argparse.Namespace) -> None:
    state = read_json(state_path())
    workspace_name = getattr(args, "workspace", None) or state.get("workspace_name")
    if not workspace_name:
        raise AirError("no workspace specified; pass --workspace or run 'workspace select <name>'")
    payload: dict[str, Any] = {
        "title": args.title,
        "visibility": args.visibility,
        "idempotency_key": args.idempotency_key or f"skill-task-{uuid.uuid4()}",
    }
    if args.command:
        payload["command"] = args.command
    result = authenticated_request("POST", f"/boards/{workspace_name}/threads", payload)
    state["thread_id"] = result["id"]
    state["thread_title"] = result["title"]
    state["workspace_name"] = workspace_name
    write_secure_json(state_path(), state)
    print(f"Dispatched task '{result['title']}' to workspace '{workspace_name}' (thread {result['id']}).")


def task_create(args: argparse.Namespace) -> None:
    payload: dict[str, Any] = {
        "title": args.title,
        "priority": getattr(args, "priority", "normal") or "normal",
    }
    if getattr(args, "workspace_id", None):
        payload["workspace_id"] = args.workspace_id
    if getattr(args, "idempotency_key", None):
        payload["idempotency_key"] = args.idempotency_key
    result = authenticated_request("POST", "/tasks", payload)
    print(f"Created task '{result.get('title')}' ({result.get('id')}). Status: {result.get('status')}")


def task_list(args: argparse.Namespace) -> None:
    status = getattr(args, "status", "all") or "all"
    result = authenticated_request("GET", f"/tasks?status={status}")
    tasks = result if isinstance(result, list) else result.get("tasks", [])
    if not tasks:
        print(f"No tasks found (filter: {status}).")
        return
    for t in tasks:
        print(f"[{t.get('status', 'queued').upper()}] {t.get('title', 'Untitled')} ({t.get('id')})")


def approval_list(args: argparse.Namespace) -> None:
    status = getattr(args, "status", "pending") or "pending"
    result = authenticated_request("GET", f"/approvals?status={status}")
    approvals = result if isinstance(result, list) else result.get("approvals", [])
    if not approvals:
        print(f"No approvals found (status: {status}).")
        return
    for a in approvals:
        print(f"[{a.get('status', 'pending').upper()}] {a.get('requested_action')} ({a.get('id')}) - Risk: {a.get('risk_level')}")


def approval_decide(args: argparse.Namespace) -> None:
    decision = getattr(args, "decision", "approved")
    payload = {
        "decision": decision,
        "reason": getattr(args, "reason", "") or "",
    }
    result = authenticated_request("POST", f"/approvals/{args.id}/decide", payload)
    print(f"Approval {args.id} decided: {result.get('status', decision)}")


def check(_args: argparse.Namespace) -> None:
    state = read_json(state_path())
    cursor = int(state.get("cursor", 0))
    result = authenticated_request("GET", f"/inbox?{urlencode({'since': cursor})}")
    notes = result.get("notes", [])
    state["cursor"] = int(result.get("cursor", cursor))
    write_secure_json(state_path(), state)
    if not notes:
        print("No new Air notes.")
        return
    for item in notes:
        print(f"[{item['kind']}] {item['body']}")


def save(args: argparse.Namespace) -> None:
    try:
        body = Path(args.body_file).read_text(encoding="utf-8").strip()
    except OSError as error:
        raise AirError(f"could not read procedure body: {error}") from error
    fields = {
        "title": args.title.strip(),
        "summary": args.summary.strip(),
        "body": body,
        "check_description": args.check.strip(),
    }
    for name, value in fields.items():
        if not value:
            raise AirError(f"{name} must not be empty")

    existing = read_json(draft_path())
    draft = {
        **fields,
        "changelog": args.changelog.strip() if args.changelog else None,
        "remote": existing.get("remote"),
        "last_published": existing.get("last_published"),
        "create_idempotency_key": existing.get("create_idempotency_key")
        or f"skill-procedure-{uuid.uuid4()}",
        "version_idempotency_key": f"skill-version-{uuid.uuid4()}",
        "publish_idempotency_key": f"skill-publish-{uuid.uuid4()}",
    }
    write_secure_json(draft_path(), draft)
    print(f"Procedure saved locally at {draft_path()}.")


def procedure_fields(draft: dict[str, Any]) -> dict[str, str]:
    return {
        "title": str(draft.get("title", "")),
        "summary": str(draft.get("summary", "")),
        "body": str(draft.get("body", "")),
        "check_description": str(draft.get("check_description", "")),
    }


def print_publish_preview(draft: dict[str, Any]) -> None:
    print("WARNING: publishing sends the full procedure body to a public server.")
    print("Air does not strip secrets or private context. Review every line below.\n")
    previous = draft.get("last_published") or {}
    old_body = str(previous.get("body", ""))
    new_body = str(draft.get("body", ""))
    diff = difflib.unified_diff(
        old_body.splitlines(keepends=True),
        new_body.splitlines(keepends=True),
        fromfile="published/body.md" if previous else "/dev/null",
        tofile="draft/body.md",
    )
    rendered = "".join(diff)
    print(rendered if rendered else "(procedure body is unchanged)")
    print("\nMetadata:")
    for name in ("title", "summary", "check_description"):
        print(f"{name}: {draft.get(name, '')}")


def post(args: argparse.Namespace) -> None:
    draft = read_json(draft_path(), required=True)
    print_publish_preview(draft)
    if not args.confirm:
        print("\nPreview only. Explicit human confirmation is required before upload.")
        return

    fields = procedure_fields(draft)
    if any(not value.strip() for value in fields.values()):
        raise AirError("the saved procedure is incomplete; run /air save again")
    remote = draft.get("remote")
    previous = draft.get("last_published")
    if not remote:
        created = authenticated_request(
            "POST",
            "/procedures",
            {**fields, "idempotency_key": draft["create_idempotency_key"]},
        )
        remote = {
            "id": created["id"],
            "slug": created["slug"],
            "current_version": created["current_version"],
        }
    elif previous != fields:
        updated = authenticated_request(
            "PATCH",
            f"/procedures/{remote['id']}",
            {
                **fields,
                "changelog": draft.get("changelog"),
                "idempotency_key": draft["version_idempotency_key"],
            },
        )
        remote["current_version"] = updated["procedure"]["current_version"]

    published = authenticated_request(
        "POST",
        f"/procedures/{remote['id']}/publish",
        {
            "visibility": args.visibility,
            "idempotency_key": draft["publish_idempotency_key"],
        },
    )
    draft["remote"] = remote
    draft["last_published"] = fields
    write_secure_json(draft_path(), draft)
    print(
        f"Published procedure {published['id']} as {published['visibility']} "
        f"(version {published['current_version']})."
    )


def record_run(args: argparse.Namespace) -> None:
    payload: dict[str, Any] = {
        "procedure_id": args.procedure_id,
        "model": args.model,
        "outcome": args.outcome,
        "attempts": args.attempts,
        "duration_ms": args.duration_ms,
        "idempotency_key": args.idempotency_key,
    }
    if args.procedure_version is not None:
        payload["procedure_version"] = args.procedure_version
    if args.thread_id:
        payload["thread_id"] = args.thread_id
    result = authenticated_request("POST", "/runs", payload)
    print(f"Recorded {result['outcome']} run {result['id']} using {result['model']}.")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description="Air skill helper")
    root.add_argument("--config-dir", help="override path to configuration directory")
    commands = root.add_subparsers(dest="command", required=True)

    login_parser = commands.add_parser("login", help="complete device login")
    login_parser.add_argument(
        "--base-url", default=os.environ.get("AIR_BASE_URL", DEFAULT_BASE_URL)
    )
    login_parser.set_defaults(handler=login)

    note_parser = commands.add_parser("note", help="write a status note")
    note_parser.add_argument("text")
    note_parser.add_argument("--title")
    note_parser.add_argument("--workspace")
    note_parser.add_argument("--idempotency-key")
    note_parser.set_defaults(handler=note)

    workspace_parser = commands.add_parser("workspace", help="manage workspace coordination")
    ws_sub = workspace_parser.add_subparsers(dest="ws_command", required=True)

    ws_list = ws_sub.add_parser("list", help="list workspaces")
    ws_list.set_defaults(handler=workspace_list)

    ws_create = ws_sub.add_parser("create", help="create a new workspace")
    ws_create.add_argument("name", help="workspace name")
    ws_create.add_argument("--description", default="", help="workspace description")
    ws_create.add_argument("--visibility", choices=("private", "public", "unlisted"), default="private")
    ws_create.set_defaults(handler=workspace_create)

    ws_edit = ws_sub.add_parser("edit", help="edit a workspace")
    ws_edit.add_argument("name", help="workspace name")
    ws_edit.add_argument("--description", help="new description")
    ws_edit.add_argument("--visibility", choices=("private", "public", "unlisted"), help="new visibility")
    ws_edit.set_defaults(handler=workspace_edit)

    ws_show = ws_sub.add_parser("show", help="inspect a workspace")
    ws_show.add_argument("name", help="workspace name")
    ws_show.set_defaults(handler=workspace_show)

    ws_sel = ws_sub.add_parser("select", help="select active workspace")
    ws_sel.add_argument("name", help="workspace name")
    ws_sel.set_defaults(handler=workspace_select)

    ws_stat = ws_sub.add_parser("status", help="show active workspace status")
    ws_stat.set_defaults(handler=workspace_status)

    ws_assign = ws_sub.add_parser("assign", help="assign an agent to a workspace")
    ws_assign.add_argument("name", help="workspace name")
    ws_assign.add_argument("--agent-id", required=True, help="agent id")
    ws_assign.add_argument("--role", default="bot", help="agent role")
    ws_assign.set_defaults(handler=workspace_assign)

    ws_task = ws_sub.add_parser("task", help="dispatch a new task to workspace")
    ws_task.add_argument("title", help="task title")
    ws_task.add_argument("--command", help="initial instruction for assigned agents")
    ws_task.add_argument("--workspace", help="workspace name (defaults to active workspace)")
    ws_task.add_argument("--visibility", choices=("private", "public", "unlisted"), default="private")
    ws_task.add_argument("--idempotency-key")
    ws_task.set_defaults(handler=workspace_task)

    agents_parser = commands.add_parser("agents", help="list caller's active connected agents")
    agents_parser.set_defaults(handler=agents_list)

    wf_parser = commands.add_parser("workflow", help="manage workflows")
    wf_sub = wf_parser.add_subparsers(dest="wf_command", required=True)

    wf_list_cmd = wf_sub.add_parser("list", help="list workflows")
    wf_list_cmd.add_argument("--workspace", help="filter by workspace")
    wf_list_cmd.set_defaults(handler=workflow_list)

    wf_show_cmd = wf_sub.add_parser("show", help="inspect a workflow")
    wf_show_cmd.add_argument("id", help="workflow id")
    wf_show_cmd.set_defaults(handler=workflow_show)

    wf_create_cmd = wf_sub.add_parser("create", help="create a new workflow")
    wf_create_cmd.add_argument("--title", required=True, help="workflow title")
    wf_create_cmd.add_argument("--summary", required=True, help="workflow summary")
    wf_create_cmd.add_argument("--body-file", help="path to instructions markdown file")
    wf_create_cmd.add_argument("--instructions", help="inline instructions text")
    wf_create_cmd.add_argument("--check", required=True, help="completion check description")
    wf_create_cmd.add_argument("--workspace", help="workspace name")
    wf_create_cmd.add_argument("--visibility", choices=("private", "public", "unlisted"), default="private")
    wf_create_cmd.add_argument("--steps-file", help="path to json file defining ordered steps")
    wf_create_cmd.add_argument("--step", action="append", help="inline step: 'title:instructions' or 'title:instructions:agent_id'")
    wf_create_cmd.add_argument("--idempotency-key")
    wf_create_cmd.set_defaults(handler=workflow_create)

    wf_edit_cmd = wf_sub.add_parser("edit", help="edit a workflow")
    wf_edit_cmd.add_argument("id", help="workflow id")
    wf_edit_cmd.add_argument("--title", help="new title")
    wf_edit_cmd.add_argument("--summary", help="new summary")
    wf_edit_cmd.add_argument("--body-file", help="path to new instructions markdown file")
    wf_edit_cmd.add_argument("--check", help="new completion check description")
    wf_edit_cmd.add_argument("--visibility", choices=("private", "public", "unlisted"), help="new visibility")
    wf_edit_cmd.add_argument("--steps-file", help="path to json file defining ordered steps")
    wf_edit_cmd.set_defaults(handler=workflow_edit)

    step_parser = commands.add_parser("step", help="check or complete assigned workflow steps")
    step_sub = step_parser.add_subparsers(dest="step_command", required=True)

    step_cur = step_sub.add_parser("current", help="check current assigned step")
    step_cur.set_defaults(handler=step_current)

    step_comp = step_sub.add_parser("complete", help="submit result for assigned step")
    step_comp.add_argument("run_id", help="run id")
    step_comp.add_argument("step_order", type=int, help="step order number")
    step_comp.add_argument("--result", help="result output text")
    step_comp.add_argument("--result-file", help="path to file with result output")
    step_comp.add_argument("--outcome", choices=("pass", "fail", "abandoned"), default="pass")
    step_comp.add_argument("--idempotency-key")
    step_comp.set_defaults(handler=step_complete)

    check_parser = commands.add_parser("check", help="fetch the inbox once")
    check_parser.set_defaults(handler=check)

    save_parser = commands.add_parser("save", help="save a local procedure draft")
    save_parser.add_argument("--title", required=True)
    save_parser.add_argument("--summary", required=True)
    save_parser.add_argument("--check", required=True)
    save_parser.add_argument("--body-file", required=True)
    save_parser.add_argument("--changelog")
    save_parser.set_defaults(handler=save)

    post_parser = commands.add_parser("post", help="preview or publish the saved procedure")
    post_parser.add_argument("--confirm", action="store_true")
    post_parser.add_argument("--visibility", choices=("public", "unlisted"), default="public")
    post_parser.set_defaults(handler=post)

    run_parser = commands.add_parser("run", help="manage workflow runs or record procedure run")
    run_sub = run_parser.add_subparsers(dest="run_command")

    run_start_cmd = run_sub.add_parser("start", help="start a workflow run")
    run_start_cmd.add_argument("workflow_id", help="workflow id")
    run_start_cmd.add_argument("--model", help="optional model identifier")
    run_start_cmd.add_argument("--idempotency-key")
    run_start_cmd.set_defaults(handler=run_start)

    run_show_cmd = run_sub.add_parser("show", help="inspect a run status and steps")
    run_show_cmd.add_argument("run_id", help="run id")
    run_show_cmd.set_defaults(handler=run_show)

    run_rec_cmd = run_sub.add_parser("record", help="record a completed procedure run")
    run_rec_cmd.add_argument("--procedure-id", required=True)
    run_rec_cmd.add_argument("--procedure-version", type=int)
    run_rec_cmd.add_argument("--model", required=True)
    run_rec_cmd.add_argument("--outcome", choices=("pass", "fail", "abandoned"), required=True)
    run_rec_cmd.add_argument("--attempts", type=int, default=1)
    run_rec_cmd.add_argument("--duration-ms", type=int, required=True)
    run_rec_cmd.add_argument("--thread-id")
    run_rec_cmd.add_argument("--idempotency-key", required=True)
    run_rec_cmd.set_defaults(handler=record_run)

    daemon_parser = commands.add_parser("daemon", help="manage 24/7 background bot daemon")
    daemon_sub = daemon_parser.add_subparsers(dest="daemon_command", required=True)

    daemon_run_cmd = daemon_sub.add_parser("run", help="run the background heartbeat listener loop")
    daemon_run_cmd.add_argument("--interval", type=int, default=30, help="heartbeat interval in seconds (default: 30)")
    daemon_run_cmd.add_argument("--runner", choices=("log", "jcode", "agy", "auto"), default="auto", help="step runner (log, jcode, agy, or auto)")
    daemon_run_cmd.add_argument("--workspace", help="working directory for headless executions")
    daemon_run_cmd.set_defaults(handler=daemon_run)

    daemon_install_cmd = daemon_sub.add_parser("install", help="install systemd user service for 24/7 background bot")
    daemon_install_cmd.add_argument("--interval", type=int, default=30, help="heartbeat interval in seconds (default: 30)")
    daemon_install_cmd.add_argument("--runner", choices=("log", "jcode", "agy", "auto"), default="auto", help="step runner to configure in the service (default: auto)")
    daemon_install_cmd.add_argument("--service-name", default=None, help="systemd service name (default: air-agent-<name>)")
    daemon_install_cmd.set_defaults(handler=daemon_install)

    daemon_status_cmd = daemon_sub.add_parser("status", help="check status of the background bot daemon")
    daemon_status_cmd.add_argument("--service-name", default=None, help="systemd service name (default: air-agent-<name>)")
    daemon_status_cmd.set_defaults(handler=daemon_status)

    listen_parser = commands.add_parser("listen", help="listen for assigned workflow steps and stay online")
    listen_parser.add_argument("--interval", type=int, default=30, help="heartbeat interval in seconds (default: 30)")
    listen_parser.add_argument("--runner", choices=("log", "jcode", "agy", "auto"), default="auto", help="step runner (log, jcode, agy, or auto)")
    listen_parser.add_argument("--workspace", help="working directory for headless executions")
    listen_parser.set_defaults(handler=daemon_run)

    task_parser = commands.add_parser("task", help="manage workspace tasks")
    task_sub = task_parser.add_subparsers(dest="task_command", required=True)

    task_create_cmd = task_sub.add_parser("create", help="create a new task")
    task_create_cmd.add_argument("title", help="task title")
    task_create_cmd.add_argument("--priority", choices=("low", "normal", "high", "urgent"), default="normal", help="priority")
    task_create_cmd.add_argument("--workspace-id", help="target workspace id")
    task_create_cmd.add_argument("--idempotency-key", help="idempotency key")
    task_create_cmd.set_defaults(handler=task_create)

    task_list_cmd = task_sub.add_parser("list", help="list tasks")
    task_list_cmd.add_argument("--status", choices=("all", "queued", "assigned", "running", "waiting_approval", "completed", "failed", "cancelled"), default="all")
    task_list_cmd.set_defaults(handler=task_list)

    approval_parser = commands.add_parser("approval", help="manage approvals")
    approval_sub = approval_parser.add_subparsers(dest="approval_command", required=True)

    approval_list_cmd = approval_sub.add_parser("list", help="list approvals")
    approval_list_cmd.add_argument("--status", choices=("pending", "approved", "rejected", "cancelled"), default="pending")
    approval_list_cmd.set_defaults(handler=approval_list)

    approval_approve_cmd = approval_sub.add_parser("approve", help="approve an action")
    approval_approve_cmd.add_argument("id", help="approval id")
    approval_approve_cmd.add_argument("--reason", default="", help="reason")
    approval_approve_cmd.set_defaults(handler=approval_decide, decision="approved")

    approval_reject_cmd = approval_sub.add_parser("reject", help="reject an action")
    approval_reject_cmd.add_argument("id", help="approval id")
    approval_reject_cmd.add_argument("--reason", default="", help="reason")
    approval_reject_cmd.set_defaults(handler=approval_decide, decision="rejected")

    return root


def main() -> int:
    args = parser().parse_args()
    if getattr(args, "config_dir", None):
        os.environ["AIR_CONFIG_DIR"] = str(Path(args.config_dir).expanduser().resolve())
    try:
        args.handler(args)
    except (AirError, KeyError, ValueError) as error:
        print(f"air: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
