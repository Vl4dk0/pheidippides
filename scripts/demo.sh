#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

if [[ ! -x "$TARGET_DIR/release/phi" || ! -x "$TARGET_DIR/release/phi-server" ]]; then
  CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release -p phi-cli -p phi-server --manifest-path "$ROOT/Cargo.toml"
fi

PHI_BIN="$TARGET_DIR/release/phi"
SERVER_BIN="$TARGET_DIR/release/phi-server"
OUT_GIF="${1:-$ROOT/docs/images/demo.gif}"

mkdir -p "$(dirname "$OUT_GIF")"

python3 - "$PHI_BIN" "$SERVER_BIN" "$OUT_GIF" <<'PY'
import hashlib
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

phi_bin = Path(sys.argv[1]).resolve()
server_bin = Path(sys.argv[2]).resolve()
out_gif = Path(sys.argv[3]).resolve()


def clean_env():
    return {k: v for k, v in os.environ.items() if not k.startswith("PHI_")}


def pick_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def capture_real_cli():
    with tempfile.TemporaryDirectory(prefix="phi-demo-") as tmp:
        work = Path(tmp).resolve()
        port = pick_port()
        cfg = work / "server.toml"
        cfg.write_text(
            f'data_dir = "data"\n'
            f'listen = "127.0.0.1:{port}"\n'
            f'allow_space_creation = "anyone"\n'
            f"[tls]\n"
            f'mode = "off"\n',
            encoding="utf-8",
        )
        url = f"http://127.0.0.1:{port}"
        log_path = work / "server.log"
        with log_path.open("w+", encoding="utf-8") as log_file:
            server_proc = subprocess.Popen(
                [str(server_bin), "--config", str(cfg)],
                cwd=work,
                env=clean_env(),
                stdout=log_file,
                stderr=log_file,
            )
            try:
                deadline = time.monotonic() + 15.0
                while True:
                    if server_proc.poll() is not None:
                        log_file.seek(0)
                        raise RuntimeError(f"phi-server exited early:\n{log_file.read()}")
                    try:
                        with urllib.request.urlopen(url + "/healthz", timeout=0.3) as resp:
                            if resp.status == 200:
                                break
                    except (urllib.error.URLError, TimeoutError):
                        pass
                    if time.monotonic() >= deadline:
                        raise RuntimeError("phi-server did not become healthy in time")
                    time.sleep(0.05)

                envs = {}
                homes = {}
                dirs = {}
                for user in ("alice", "bob"):
                    home = work / "home" / user
                    proj = home / "engine"
                    proj.mkdir(parents=True, exist_ok=True)
                    homes[user] = home
                    dirs[user] = proj
                    envs[user] = clean_env() | {
                        "HOME": str(home),
                        "USER": user,
                        "LOGNAME": user,
                        "XDG_CONFIG_HOME": str(home / ".config"),
                        "XDG_CACHE_HOME": str(home / ".cache"),
                    }

                def run_phi(user, *args):
                    res = subprocess.run(
                        [str(phi_bin), *args],
                        cwd=dirs[user],
                        env=envs[user],
                        capture_output=True,
                        text=True,
                        timeout=20,
                        check=False,
                    )
                    if res.returncode != 0:
                        raise RuntimeError(
                            f"phi {args} failed for {user} (exit {res.returncode}):\n{res.stderr}"
                        )
                    return res.stdout, res.stderr

                create_out, _ = run_phi("alice", "space", "create", "engine", "--server", url)
                invite_out, invite_err = run_phi("alice", "invite", "--note", "for bob")
                invite_link = invite_out.strip()
                join_out, _ = run_phi("bob", "join", invite_link, "--name", "bob")
                run_phi("alice", "init", "engine")
                run_phi("bob", "init", "engine")

                # Drain any initial setup events before the recorded agent exchange.
                run_phi("alice", "--as", "main", "wait", "1")
                run_phi("bob", "--as", "main", "wait", "1")

                req_title = "Frame budget?"
                req_body = "What frame budget does the renderer target? Please attach config."
                send_out, _ = run_phi(
                    "alice",
                    "--as",
                    "main",
                    "send",
                    "@bob",
                    req_title,
                    req_body,
                )
                thread_match = re.search(r"^thread (\S+)", send_out)
                if not thread_match:
                    raise RuntimeError(f"could not parse thread id from: {send_out}")
                thread_id = thread_match.group(1)

                bob_wait_out, _ = run_phi("bob", "--as", "main", "wait", "5")
                bob_claim_out, _ = run_phi("bob", "--as", "main", "react", f"{thread_id}#1", "claim")

                attachment_text = (
                    'target_hz = 120\n'
                    'frame_budget_ms = 8.33\n'
                    'gpu_budget_ms = 6.50\n'
                    'vsync = "adaptive"\n'
                )
                (dirs["bob"] / "renderer.toml").write_text(attachment_text, encoding="utf-8")

                reply_body = "Targets 8.33 ms (120 Hz) with a 6.50 ms GPU budget. Config attached."
                bob_reply_out, _ = run_phi(
                    "bob",
                    "--as",
                    "main",
                    "reply",
                    thread_id,
                    reply_body,
                    "--file",
                    "renderer.toml",
                )

                alice_wait_out, _ = run_phi("alice", "--as", "main", "wait", "5")

                downloaded = list(homes["alice"].rglob("renderer.toml"))
                if not downloaded or downloaded[0].read_text(encoding="utf-8") != attachment_text:
                    raise RuntimeError("Alice did not receive intact renderer.toml attachment")

                def sanitize(text):
                    for user in ("alice", "bob"):
                        h_raw = str(homes[user])
                        h_res = str(homes[user].resolve())
                        for prefix in sorted({h_raw, h_res}, key=len, reverse=True):
                            text = text.replace(f"{prefix}/Library/Caches", f"/home/{user}/.cache")
                            text = text.replace(
                                f"{prefix}/Library/Application Support", f"/home/{user}/.config"
                            )
                            text = text.replace(prefix, f"/home/{user}")
                    text = text.replace(f"127.0.0.1:{port}", "127.0.0.1:8443")
                    text = re.sub(r"(phis?://[^\s#'\"]+)#[A-Za-z0-9_-]+", r"\1#…", text)
                    return text

                data = {
                    "thread_id": thread_id,
                    "invite_note": sanitize(invite_err),
                    "invite_link": sanitize(invite_out),
                    "join_out": sanitize(join_out),
                    "send_out": sanitize(send_out),
                    "bob_wait_out": sanitize(bob_wait_out),
                    "bob_claim_out": sanitize(bob_claim_out),
                    "bob_reply_out": sanitize(bob_reply_out),
                    "alice_wait_out": sanitize(alice_wait_out),
                }
                for key, val in data.items():
                    if any(leak in val for leak in (str(work), "/var/folders", "/private/tmp", "/Users/")):
                        raise RuntimeError(f"Unsanitized path leaked in {key}: {val}")
                    if re.search(r"phis?://[^\s#'\"]+#[A-Za-z0-9_-]{4,}", val):
                        raise RuntimeError(f"Unsanitized secret leaked in {key}: {val}")
                return data
            finally:
                server_proc.terminate()
                try:
                    server_proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server_proc.kill()
                    server_proc.wait(timeout=5)


def build_cast(data, cast_path):
    """Write an asciinema v2 .cast file representing two 80-column panes side by side."""
    pane_w = 80
    total_w = pane_w * 2 + 3  # 80 + " │ " + 80 = 163
    total_h = 27  # 2 header rows + 25 content rows

    header = {
        "version": 2,
        "width": total_w,
        "height": total_h,
        "timestamp": 1760960000,
        "title": "Pheidippides — two AI agents talking through phi",
        "env": {"TERM": "xterm-256color", "SHELL": "/bin/zsh"},
        "theme": {
            "fg": "#e3e9e3",
            "bg": "#171c1a",
            "palette": "#171c1a:#f19b9b:#9ac6ac:#e0af68:#7aa2f7:#9ac6ac:#9ac6ac:#e3e9e3:#93a298:#f19b9b:#9ac6ac:#e0af68:#7aa2f7:#9ac6ac:#9ac6ac:#e3e9e3",
        },
    }

    events = []
    t = 0.0

    def emit(dt, text):
        nonlocal t
        t = round(t + dt, 4)
        events.append([t, "o", text])

    def goto(row, col):
        return f"\x1b[{row};{col}H"

    # ANSI SGR codes mapped in our theme:
    # 0 = reset (#e3e9e3), 32 = accent (#9ac6ac), 33 = claim/untrusted (#e0af68),
    # 37 = output text (#c8d4cc), 90 = muted/border (#93a298), 38;5;238 = line (#2c3730)
    C_RESET = "\x1b[0m"
    C_ACCENT = "\x1b[32m"
    C_WARN = "\x1b[33m"
    C_OUT = "\x1b[37m"
    C_MUTED = "\x1b[90m"
    C_LINE = "\x1b[38;5;238m"

    pane_row = {"alice": 3, "bob": 3}
    pane_col = {"alice": 1, "bob": 1}

    def base_col(side):
        return 1 if side == "alice" else pane_w + 4

    def draw_chrome(active_side=None):
        buf = ["\x1b[?25l"]
        left_dot = f"{C_ACCENT}●" if active_side == "alice" else f"{C_MUTED}○"
        right_dot = f"{C_ACCENT}●" if active_side == "bob" else f"{C_MUTED}○"
        left_title = (
            f" {left_dot} {C_ACCENT}alice/main{C_MUTED} (agent) · /home/alice/engine"
        )
        right_title = (
            f" {right_dot} {C_ACCENT}bob/main{C_MUTED} (agent) · /home/bob/engine"
        )
        buf.append(goto(1, 1) + left_title)
        buf.append(goto(1, pane_w + 2) + f"{C_LINE}│")
        buf.append(goto(1, pane_w + 4) + right_title)
        divider_row = "─" * (pane_w + 1) + "┼" + "─" * (pane_w + 1)
        buf.append(goto(2, 1) + f"{C_LINE}{divider_row}{C_RESET}")
        for r in range(3, total_h + 1):
            buf.append(goto(r, pane_w + 2) + f"{C_LINE}│{C_RESET}")
        return "".join(buf)

    def write_in_pane(side, text, color=C_OUT):
        buf = [color]
        bcol = base_col(side)
        for ch in text:
            if ch == "\n":
                pane_row[side] += 1
                pane_col[side] = 1
                continue
            if pane_col[side] > pane_w:
                pane_row[side] += 1
                pane_col[side] = 1
            buf.append(goto(pane_row[side], bcol + pane_col[side] - 1) + ch)
            pane_col[side] += 1
        buf.append(C_RESET)
        return "".join(buf)

    def focus_pane(side):
        bcol = base_col(side)
        return (
            draw_chrome(side)
            + goto(pane_row[side], bcol + pane_col[side] - 1)
            + "\x1b[?25h"
        )

    def prompt_in_pane(side):
        bcol = base_col(side)
        out = write_in_pane(side, "$ ", color=C_ACCENT)
        out += goto(pane_row[side], bcol + pane_col[side] - 1)
        return out

    def type_command(side, cmd, char_delay=0.028):
        emit(0.15, focus_pane(side))
        bcol = base_col(side)
        for i, ch in enumerate(cmd):
            if ch == "\n":
                pane_row[side] += 1
                pane_col[side] = 1
                emit(
                    0.08,
                    goto(pane_row[side], bcol + pane_col[side] - 1),
                )
                continue
            if pane_col[side] > pane_w:
                pane_row[side] += 1
                pane_col[side] = 1
            jitter = ((i * 7) % 5 - 2) * 0.004
            delay = max(0.014, char_delay + jitter)
            if ch in (" ", '"', "\\"):
                delay += 0.018
            s = (
                goto(pane_row[side], bcol + pane_col[side] - 1)
                + C_RESET
                + ch
                + goto(pane_row[side], bcol + pane_col[side])
            )
            pane_col[side] += 1
            emit(delay, s)
        # Press Enter
        pane_row[side] += 1
        pane_col[side] = 1
        emit(0.18, goto(pane_row[side], bcol))

    def print_output(side, raw_output, pause_before=0.22):
        chunks = []
        for line in raw_output.splitlines(keepends=True):
            stripped = line.rstrip("\n")
            if stripped.startswith("<UNTRUSTED") or stripped.startswith("</UNTRUSTED>"):
                col = C_WARN
            elif stripped.startswith("── #"):
                col = C_ACCENT
            elif stripped.startswith(("participants:", "next reply:", "reaction context", "invite note:")):
                col = C_MUTED
            else:
                col = C_OUT
            chunks.append(write_in_pane(side, line, color=col))
        bcol = base_col(side)
        chunks.append(goto(pane_row[side], bcol + pane_col[side] - 1))
        emit(pause_before, "".join(chunks))

    # Initial frame: chrome + trimmed invite/join setup already completed at top of panes
    init_buf = [draw_chrome("alice")]
    init_buf.append(write_in_pane("alice", "$ ", C_ACCENT))
    init_buf.append(write_in_pane("alice", 'phi invite --note "for bob"\n', C_RESET))
    init_buf.append(write_in_pane("alice", data["invite_note"], C_MUTED))
    init_buf.append(write_in_pane("alice", data["invite_link"] + "\n", C_OUT))
    init_buf.append(prompt_in_pane("alice"))

    invite_trimmed = data["invite_link"].strip()
    init_buf.append(write_in_pane("bob", "$ ", C_ACCENT))
    init_buf.append(
        write_in_pane("bob", f"phi join '{invite_trimmed}' --name bob\n", C_RESET)
    )
    init_buf.append(write_in_pane("bob", data["join_out"] + "\n", C_OUT))
    init_buf.append(prompt_in_pane("bob"))
    init_buf.append(focus_pane("alice"))
    emit(0.0, "".join(init_buf))
    emit(0.7, "")

    tid = data["thread_id"]

    # 1. Alice sends request to Bob
    cmd_send = (
        'phi --as main send @bob "Frame budget?" \\\n'
        '  "What frame budget does the renderer target? Please attach config."'
    )
    type_command("alice", cmd_send, char_delay=0.026)
    print_output("alice", data["send_out"] + "\n", pause_before=0.25)
    emit(0.15, prompt_in_pane("alice"))

    # 2. Bob runs `phi wait` and receives Alice's request
    emit(0.45, focus_pane("bob"))
    type_command("bob", "phi --as main wait", char_delay=0.035)
    print_output("bob", data["bob_wait_out"] + "\n", pause_before=0.35)
    emit(0.8, prompt_in_pane("bob"))

    # 3. Bob claims the request
    type_command("bob", f"phi --as main react {tid}#1 claim", char_delay=0.028)
    print_output("bob", data["bob_claim_out"] + "\n", pause_before=0.22)
    emit(0.45, prompt_in_pane("bob"))

    # 4. Bob replies with renderer.toml attachment
    cmd_reply = (
        f"phi --as main reply {tid} --file renderer.toml \\\n"
        '  "Targets 8.33 ms (120 Hz) with a 6.50 ms GPU budget. Config attached."'
    )
    type_command("bob", cmd_reply, char_delay=0.025)
    print_output("bob", data["bob_reply_out"], pause_before=0.28)
    emit(0.15, prompt_in_pane("bob"))

    # 5. Alice runs `phi wait` and receives claim + reply + attachment path
    emit(0.55, focus_pane("alice"))
    type_command("alice", "phi --as main wait", char_delay=0.035)
    print_output("alice", data["alice_wait_out"], pause_before=0.35)
    emit(0.15, prompt_in_pane("alice"))

    # Hold final frame so viewer can read the complete exchange
    emit(3.2, "\x1b[?25h")

    with cast_path.open("w", encoding="utf-8") as f:
        f.write(json.dumps(header, ensure_ascii=False) + "\n")
        for ev in events:
            f.write(json.dumps(ev, ensure_ascii=False) + "\n")

    return header, events


def render_cast_to_gif(cast_path, gif_path):
    """Render an asciinema v2 .cast file to a palette-optimized GIF using Pillow + ffmpeg."""
    lines = cast_path.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0])
    events = [json.loads(line) for line in lines[1:] if line.strip()]

    cols = header["width"]
    rows = header["height"]

    # Theme matching apps/gui/src/style.css dark mode
    BG = (23, 28, 26)         # #171c1a --bg
    HEADER_BG = (20, 26, 23)  # #141a17 --rail
    FG_TEXT = (227, 233, 227) # #e3e9e3 --text
    FG_OUT = (202, 214, 206)  # soft text for stdout
    FG_MUTED = (147, 162, 152)# #93a298 --muted
    FG_ACCENT = (154, 198, 172)# #9ac6ac --accent
    FG_WARN = (224, 175, 104) # #e0af68 --claim
    FG_LINE = (44, 55, 48)    # #2c3730 --line

    font_candidates = [
        "/System/Library/Fonts/SFNSMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    ]
    font_path = next((p for p in font_candidates if Path(p).is_file()), None)
    if font_path is None:
        raise RuntimeError("No suitable monospace font found")

    font_size = 15
    font = ImageFont.truetype(font_path, font_size)
    cell_w = font.getlength("M")
    cell_h = 22
    pad_x = 16
    pad_y = 12
    img_w = int(round(cols * cell_w + pad_x * 2))
    img_h = rows * cell_h + pad_y * 2

    grid_ch = [[" " for _ in range(cols)] for _ in range(rows)]
    grid_fg = [[FG_TEXT for _ in range(cols)] for _ in range(rows)]
    cur_r = 0
    cur_c = 0
    cur_fg = FG_TEXT
    cursor_visible = False

    ansi_re = re.compile(r"\x1b\[([0-9;?]*)([A-Za-z])")

    def feed(data):
        nonlocal cur_r, cur_c, cur_fg, cursor_visible
        pos = 0
        for m in ansi_re.finditer(data):
            chunk = data[pos : m.start()]
            for ch in chunk:
                if ch == "\r":
                    cur_c = 0
                elif ch == "\n":
                    cur_r = min(rows - 1, cur_r + 1)
                else:
                    if 0 <= cur_r < rows and 0 <= cur_c < cols:
                        grid_ch[cur_r][cur_c] = ch
                        grid_fg[cur_r][cur_c] = cur_fg
                    cur_c = min(cols - 1, cur_c + 1)
            params, cmd = m.group(1), m.group(2)
            if cmd == "H":
                parts = params.split(";") if params else ["1", "1"]
                r = int(parts[0] or 1) - 1
                c = int(parts[1] if len(parts) > 1 and parts[1] else 1) - 1
                cur_r = max(0, min(rows - 1, r))
                cur_c = max(0, min(cols - 1, c))
            elif cmd == "m":
                code = params or "0"
                if code == "0":
                    cur_fg = FG_TEXT
                elif code == "32":
                    cur_fg = FG_ACCENT
                elif code == "33":
                    cur_fg = FG_WARN
                elif code == "37":
                    cur_fg = FG_OUT
                elif code == "90":
                    cur_fg = FG_MUTED
                elif code == "38;5;238":
                    cur_fg = FG_LINE
            elif cmd == "l" and params == "?25":
                cursor_visible = False
            elif cmd == "h" and params == "?25":
                cursor_visible = True
            pos = m.end()
        tail = data[pos:]
        for ch in tail:
            if ch == "\r":
                cur_c = 0
            elif ch == "\n":
                cur_r = min(rows - 1, cur_r + 1)
            else:
                if 0 <= cur_r < rows and 0 <= cur_c < cols:
                    grid_ch[cur_r][cur_c] = ch
                    grid_fg[cur_r][cur_c] = cur_fg
                cur_c = min(cols - 1, cur_c + 1)

    def render_snapshot():
        im = Image.new("RGB", (img_w, img_h), BG)
        draw = ImageDraw.Draw(im)

        # Header band background and crisp structural lines
        header_bottom = pad_y + cell_h + 10
        draw.rectangle([0, 0, img_w, header_bottom], fill=HEADER_BG)
        draw.line([(0, header_bottom), (img_w, header_bottom)], fill=FG_LINE, width=1)
        div_x = int(round(pad_x + 81.5 * cell_w))
        draw.line([(div_x, 0), (div_x, img_h)], fill=FG_LINE, width=1)
        # Outer border
        draw.rectangle([0, 0, img_w - 1, img_h - 1], outline=FG_LINE, width=1)

        for r in range(rows):
            if r == 1:
                # Row 1 (0-indexed) is the horizontal border drawn cleanly with draw.line above
                continue
            y = pad_y + r * cell_h + (2 if r == 0 else 0)
            # Group contiguous characters of the same color on each side of the divider
            c = 0
            while c < cols:
                if c == 81 or grid_ch[r][c] == " ":
                    c += 1
                    continue
                fg = grid_fg[r][c]
                start_c = c
                chars = []
                while c < cols and c != 81 and grid_fg[r][c] == fg:
                    if grid_ch[r][c] == " " and (c + 1 >= cols or grid_ch[r][c + 1] == " "):
                        break
                    chars.append(grid_ch[r][c])
                    c += 1
                x = int(round(pad_x + start_c * cell_w))
                draw.text((x, y), "".join(chars), font=font, fill=fg)

        if cursor_visible and 0 <= cur_r < rows and 0 <= cur_c < cols:
            cx = int(round(pad_x + cur_c * cell_w))
            cw_int = max(7, int(round(cell_w)))
            cy = pad_y + cur_r * cell_h + 2
            draw.rectangle([cx, cy, cx + cw_int - 1, cy + cell_h - 3], fill=FG_ACCENT)

        return im

    # Group events into 40ms (25 fps) buckets so GIF frame delays are >= 4 centiseconds
    quantized = []
    idx = 0
    n = len(events)
    while idx < n:
        t0 = events[idx][0]
        feed(events[idx][2])
        idx += 1
        while idx < n and (events[idx][0] - t0) < 0.04:
            feed(events[idx][2])
            idx += 1
        if idx < n:
            dur = max(0.04, round(events[idx][0] - t0, 3))
        else:
            dur = 0.5
        quantized.append((dur, render_snapshot()))

    with tempfile.TemporaryDirectory(prefix="phi-gif-frames-") as ftmp:
        fdir = Path(ftmp)
        concat_lines = ["ffconcat version 1.0"]
        for i, (dur, im) in enumerate(quantized):
            fname = f"frame_{i:04d}.png"
            im.save(fdir / fname)
            concat_lines.append(f"file '{fname}'")
            concat_lines.append(f"duration {dur:.3f}")
        # Repeat last file per ffmpeg concat demuxer spec
        concat_lines.append(f"file 'frame_{len(quantized) - 1:04d}.png'")
        concat_file = fdir / "frames.ffconcat"
        concat_file.write_text("\n".join(concat_lines) + "\n", encoding="utf-8")

        palette = fdir / "palette.png"
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                str(concat_file),
                "-vf",
                "palettegen=max_colors=64:stats_mode=full",
                str(palette),
            ],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                str(concat_file),
                "-i",
                str(palette),
                "-lavfi",
                "paletteuse=dither=none:diff_mode=rectangle",
                "-loop",
                "0",
                str(gif_path),
            ],
            check=True,
            capture_output=True,
        )


def main():
    data = capture_real_cli()
    with tempfile.TemporaryDirectory(prefix="phi-cast-") as ctmp:
        cast_path = Path(ctmp) / "demo.cast"
        _, events = build_cast(data, cast_path)
        render_cast_to_gif(cast_path, out_gif)
        total_duration = events[-1][0]
        size_bytes = out_gif.stat().st_size
        print(
            f"Rendered {out_gif} ({size_bytes / 1024:.1f} KiB, {total_duration:.1f}s, {len(events)} cast events)"
        )


if __name__ == "__main__":
    main()
PY
