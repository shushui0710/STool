#!/usr/bin/env bash
# 反修改保护诊断的真机冒烟测试：起一个受控靶子进程，用 stool-cli guard 诊断。
# 用法: bash scripts/_guard_smoke.sh
set -u

PY="C:/Users/86136/.workbuddy/binaries/python/versions/3.13.12/python.exe"
CLI="D:/STool/stool-rs/target/debug/stool-cli.exe"
cd /d/STool || exit 1

run_mode() {
  local mode="$1"; shift
  local log="D:/STool/.guard_smoke_$mode.log"
  echo "===================== MODE=$mode ====================="
  "$PY" scripts/mock_mem_guard.py "$mode" >"$log" 2>&1 &
  local helper=$!
  # 等靶子打印出 PID=... ADDR=...
  local line="" i
  for i in $(seq 1 40); do
    line=$(grep -m1 '^PID=' "$log" 2>/dev/null || true)
    [ -n "$line" ] && break
    sleep 0.25
  done
  if [ -z "$line" ]; then
    echo "!! 靶子没起来，日志："; cat "$log"
    kill "$helper" 2>/dev/null || true
    return
  fi
  local pid addr
  pid=$(echo "$line" | sed -n 's/.*PID=\([0-9]*\).*/\1/p')
  addr=$(echo "$line" | sed -n 's/.*ADDR=\(0x[0-9a-fA-F]*\).*/\1/p')
  echo "靶子: $line"
  echo "--- stool-cli guard $pid --opt:addr=$addr --opt:type=i32 ---"
  "$CLI" guard "$pid" --opt:addr="$addr" --opt:type=i32
  echo "--- 退出码 $? ---"
  kill "$helper" 2>/dev/null || true
  sleep 0.3
  if kill -0 "$helper" 2>/dev/null; then
    taskkill //PID "$pid" //F >/dev/null 2>&1 || true
  fi
  rm -f "$log"
  echo
}

echo "===== guard-regions 自测（拿靶子进程当样本）====="
"$PY" scripts/mock_mem_guard.py none >D:/STool/.guard_smoke_reg.log 2>&1 &
REG_HELPER=$!
sleep 1.2
REG_PID=$(sed -n 's/.*PID=\([0-9]*\).*/\1/p' D:/STool/.guard_smoke_reg.log | head -1)
"$CLI" guard-regions "$REG_PID" --opt:max=12
kill "$REG_HELPER" 2>/dev/null || true
rm -f D:/STool/.guard_smoke_reg.log
echo

for m in none periodic fast readonly constant; do
  run_mode "$m"
done

echo "===== 错误路径 ====="
echo "-- 不带地址:"; "$CLI" guard 1234 --opt:type=i32; echo "退出码 $?"
echo "-- 不存在的进程名:"; "$CLI" guard --opt:name=__no_such_game__.exe --opt:addr=0x1000; echo "退出码 $?"
echo "-- 非法地址:"; "$CLI" guard 1234 --opt:addr=0xzz --opt:type=i32; echo "退出码 $?"
echo "-- 文本类型:"; "$CLI" guard 1234 --opt:addr=0x1000 --opt:type=utf8; echo "退出码 $?"
echo "-- 参数不足:"; "$CLI" guard; echo "退出码 $?"
