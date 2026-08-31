#!/bin/bash
# Sample system + per-app memory every N seconds, and dump a detailed snapshot
# when something crosses a threshold. Built to catch the intermittent
# "everything eats all the RAM and the machine hangs" event in the act.
#
#   memwatch.sh [interval_seconds] [--foreground]
#
# Tethys launches this at boot (src-tauri/src/memwatch.rs). It is a singleton:
# a second invocation while one is already sampling exits immediately, so
# starting Tethys repeatedly (or launching by hand) never stacks samplers.
#
# Output: ~/memwatch/samples.tsv    one row per sample
#         ~/memwatch/snap-<ts>.txt  detail dump when a threshold trips
#         ~/memwatch/memwatch.pid   pid of the live sampler
#
# Memory is `phys_footprint` — the number Activity Monitor shows — read from
# `top`, NOT the `ps rss` this script used to use. RSS excludes compressed and
# swapped pages, so it understates by 2-3x exactly when the machine is under
# the pressure we're trying to catch.

set -u

INTERVAL=20
FOREGROUND=0
for arg in "$@"; do
  case "$arg" in
    --foreground) FOREGROUND=1 ;;
    ''|*[!0-9]*)  ;;
    *)            INTERVAL="$arg" ;;
  esac
done

OUT="$HOME/memwatch"
SAMPLES="$OUT/samples.tsv"
PIDFILE="$OUT/memwatch.pid"

# A single process at this size is the symptom we're chasing, and no threshold
# on a *named* app would have caught it — the point is to trip on whoever it is.
PROC_ALARM_MB="${PROC_ALARM_MB:-8000}"
SWAP_ALARM_MB="${SWAP_ALARM_MB:-12000}"
FREE_ALARM_MB="${FREE_ALARM_MB:-250}"
COOLDOWN="${COOLDOWN:-300}"

HEADER=$'ts\tfree_mb\tcompressor_mb\tswap_used_mb\tproc_total_mb\titerm_mb\tcode_mb\tchrome_mb\tclaude_mb\tclaude_kids_mb\ttethys_mb\twebview_mb\tdevstack_mb\tdockervm_mb\tnprocs\tbiggest_mb\ttop1\ttop2\ttop3'

mkdir -p "$OUT"

# --- singleton -------------------------------------------------------------
if [ -f "$PIDFILE" ]; then
  old=$(cat "$PIDFILE" 2>/dev/null || true)
  if [ -n "${old:-}" ] && kill -0 "$old" 2>/dev/null &&
     ps -o command= -p "$old" 2>/dev/null | grep -q memwatch; then
    echo "[memwatch] already sampling as pid $old" >&2
    exit 0
  fi
  rm -f "$PIDFILE"
fi

# --- rotate a samples file written by the old RSS-based script -------------
# The columns changed meaning as well as name, so mixing them in one file
# would silently corrupt any later awk over it.
if [ -s "$SAMPLES" ] && [ "$(head -1 "$SAMPLES")" != "$HEADER" ]; then
  mv "$SAMPLES" "$SAMPLES.$(date +%Y%m%d-%H%M%S).rss.old"
fi
[ -s "$SAMPLES" ] || printf '%s\n' "$HEADER" >"$SAMPLES"

# One sample: join `ps` (pid, ppid, full argv) against `top` (pid,
# phys_footprint) and bucket every process on the machine. Two calls and one
# awk, rather than the old script's five `ps | grep` subshells, so every column
# in a row describes the same instant.
sample_row() {
  {
    ps -Ao pid=,ppid=,command=
    echo "---SPLIT---"
    top -l 1 -stats pid,mem
  } | awk -v ts="$(date +%Y-%m-%dT%H:%M:%S)" '
    function ancestor(p, mark,   q, hops) {
      q = ppid[p]; hops = 0
      while (q != "" && q != "0" && q != "1" && hops < 25) {
        if (q in mark) return 1
        q = ppid[q]; hops++
      }
      return 0
    }
    function trim(s) { gsub(/\t/, " ", s); return substr(s, 1, 70) }

    BEGIN { mode = 1 }
    /^---SPLIT---$/ { mode = 2; next }

    mode == 1 && $1 ~ /^[0-9]+$/ {
      line = $0
      sub(/^[ \t]*[0-9]+[ \t]+[0-9]+[ \t]+/, "", line)
      cmd[$1] = line; ppid[$1] = $2; n++
      next
    }

    # top prints "<pid> <mem>" with a K/M/G suffix.
    mode == 2 && $1 ~ /^[0-9]+$/ && NF >= 2 {
      v = $2; u = substr(v, length(v), 1); x = v + 0
      if (u == "K")      x = x / 1024
      else if (u == "G") x = x * 1024
      else if (u != "M") x = x / 1048576   # unitless == bytes
      mb[$1] = x
      next
    }

    END {
      for (p in cmd) {
        argv0 = cmd[p]; sub(/ .*/, "", argv0); sub(/.*\//, "", argv0)
        if (argv0 == "claude")               claude_pid[p] = 1
        if (argv0 == "tethys")               tethys_pid[p] = 1
        if (cmd[p] ~ /pnpm tauri dev/)       dev_pid[p] = 1
      }

      for (p in cmd) {
        m = (p in mb) ? mb[p] : 0
        total += m
        if (m > biggest) { biggest = m; biggest_pid = p }
        by_mb[p] = m

        is_tethys = (p in tethys_pid) || ancestor(p, tethys_pid)

        if (p in claude_pid)                          claude += m
        else if (ancestor(p, claude_pid))             kids   += m
        else if (is_tethys)                           tethys += m
        else if (cmd[p] ~ /com\.apple\.WebKit\.WebContent/) webview += m
        # A session running tests under docker compose spends its memory in
        # the Docker VM, a launchd child that descends from no claude process,
        # so it would otherwise be invisible in every per-app column here.
        else if (cmd[p] ~ /Virtualization\.framework|Docker\.app|com\.docker/) docker += m
        else if ((p in dev_pid) || ancestor(p, dev_pid))    devstack += m
        else if (cmd[p] ~ /iTerm\.app\/Contents\/MacOS/)    iterm += m
        else if (cmd[p] ~ /Visual Studio Code\.app/)        code += m
        else if (cmd[p] ~ /Google Chrome\.app/)             chrome += m
      }

      # Three largest processes, with enough of the command line to identify
      # them later. The old script cut this at the first whitespace, which made
      # every row say "/Applications/Google".
      for (p in by_mb) {
        if (by_mb[p] > t1v) { t3v=t2v; t3=t2; t2v=t1v; t2=t1; t1v=by_mb[p]; t1=p }
        else if (by_mb[p] > t2v) { t3v=t2v; t3=t2; t2v=by_mb[p]; t2=p }
        else if (by_mb[p] > t3v) { t3v=by_mb[p]; t3=p }
      }

      printf "%s\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\t%d\t%.0f\t%s\t%s\t%s\n",
        ts, FREE, COMPRESSOR, SWAP, total,
        iterm, code, chrome, claude, kids, tethys, webview, devstack, docker,
        n, biggest,
        sprintf("%.0fMB %s", t1v, trim(cmd[t1])),
        sprintf("%.0fMB %s", t2v, trim(cmd[t2])),
        sprintf("%.0fMB %s", t3v, trim(cmd[t3]))
    }
  ' FREE="$1" COMPRESSOR="$2" SWAP="$3"
}

snapshot() {
  local why="$1" row="$2" snap
  snap="$OUT/snap-$(date +%Y%m%d-%H%M%S).txt"
  {
    echo "=== TRIPPED: $why ==="
    echo "$HEADER"
    echo "$row"
    echo; echo "=== top 30 by phys_footprint ==="
    top -l 1 -stats pid,mem,command -o mem -n 30 2>/dev/null | tail -32
    echo; echo "=== full argv of the top 30 ==="
    top -l 1 -stats pid,mem -o mem -n 30 2>/dev/null |
      awk '$1 ~ /^[0-9]+$/ {print $1}' |
      while read -r p; do ps -o pid=,ppid=,etime=,command= -p "$p" 2>/dev/null | cut -c1-160; done
    echo; echo "=== claude processes and everything they spawned ==="
    # The hypothesis this column exists to test: a session running a big test
    # suite. Descendants are what cost the memory, not `claude` itself.
    ps -Ao pid=,ppid=,etime=,command= | awk '
      { pid[$1]=$1; pp[$1]=$2; line[$1]=$0
        a=$4; sub(/.*\//,"",a); if (a=="claude") cl[$1]=1 }
      END { for (p in pid) { q=pp[p]; h=0
              while (q!="" && q!="0" && q!="1" && h<25) {
                if (q in cl) { print line[p]; break }
                q=pp[q]; h++ }
              if (p in cl) print line[p] } }' | sort -k2 -n | cut -c1-160
    echo; echo "=== vm_stat ==="; vm_stat
    echo; echo "=== swap ==="; sysctl vm.swapusage
    echo; echo "=== every tty and what is running on it ==="
    ps -Ao pid,ppid,tty,command | grep ttys | grep -v grep | cut -c1-160
    echo; echo "=== vmmap -summary of the biggest process ==="
    bp=$(top -l 1 -stats pid,mem -o mem -n 1 2>/dev/null | awk '$1 ~ /^[0-9]+$/ {print $1; exit}')
    if [ -n "${bp:-}" ]; then
      ps -o pid=,command= -p "$bp" | cut -c1-160
      vmmap -summary "$bp" 2>&1 | head -40
    fi
    echo; echo "=== tethys log tail ==="
    tail -80 "$HOME/Library/Application Support/app.tethys.dev/logs/tethys.log.$(date +%Y-%m-%d)" 2>/dev/null
  } >"$snap"
  echo "[memwatch] $why -> $snap" >&2
}

main_loop() {
  trap 'rm -f "$PIDFILE"' EXIT
  local pagesize last_snap=0 biggest_col
  pagesize=$(sysctl -n hw.pagesize)
  biggest_col=$(printf '%s' "$HEADER" |
    awk -F'\t' '{for (i=1;i<=NF;i++) if ($i=="biggest_mb") print i}')

  while :; do
    vs=$(vm_stat)
    free_mb=$(echo "$vs" | awk -v s="$pagesize" '/Pages free/{gsub(/\./,"");printf "%.0f", $3*s/1048576}')
    comp_mb=$(echo "$vs" | awk -v s="$pagesize" '/occupied by compressor/{gsub(/\./,"");printf "%.0f", $5*s/1048576}')
    swap_mb=$(sysctl -n vm.swapusage | awk '{gsub(/M/,"",$6); printf "%.0f", $6}')

    row=$(sample_row "$free_mb" "$comp_mb" "$swap_mb")
    printf '%s\n' "$row" >>"$SAMPLES"

    biggest=$(printf '%s' "$row" | cut -f"$biggest_col")
    now=$(date +%s)
    why=""
    if [ "${biggest:-0}" -gt "$PROC_ALARM_MB" ]; then
      why="single process at ${biggest}MB"
    elif [ "${swap_mb:-0}" -gt "$SWAP_ALARM_MB" ]; then
      why="swap at ${swap_mb}MB"
    elif [ "${free_mb:-99999}" -lt "$FREE_ALARM_MB" ]; then
      why="free memory at ${free_mb}MB"
    fi

    if [ -n "$why" ] && [ $((now - last_snap)) -gt "$COOLDOWN" ]; then
      last_snap=$now
      snapshot "$why" "$row"
    fi

    sleep "$INTERVAL"
  done
}

if [ "$FOREGROUND" = 1 ]; then
  echo $$ >"$PIDFILE"
  main_loop
else
  # Detach into a background subshell that ignores the terminal's signals, so
  # the sampler survives Ctrl-C on the `pnpm tauri dev` that started Tethys —
  # and survives Tethys itself dying, which is when the aftermath matters most.
  # SIGTERM is deliberately left alone: `pkill -f memwatch.sh` still works.
  ( trap '' HUP INT; main_loop >/dev/null 2>>"$OUT/memwatch.err" ) &
  echo $! >"$PIDFILE"
  echo "[memwatch] sampling every ${INTERVAL}s -> $SAMPLES (pid $!)" >&2
fi
