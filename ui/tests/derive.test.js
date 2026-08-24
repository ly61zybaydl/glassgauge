import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { deriveWindow, deriveAll, deriveStatus, resetText, tightest, limitsMatchAccount, fmtUsd, fmtAmount } from "../derive.js";

const fixture = JSON.parse(
  new TextDecoder().decode(readFileSync(new URL("./fixtures/limits.json", import.meta.url))),
);

test("fmtUsd：units → 估算美元（普通 200 units=$1，Fable 480）", () => {
  assert.equal(fmtUsd(156800, 200), "$784"); // 普通：156800/200
  assert.equal(fmtUsd(560000, 200), "$2,800"); // ≥1000 千分位
  assert.equal(fmtUsd(296800, 480), "$618"); // Fable 费率 200×2.4
  assert.equal(fmtUsd(50, 200), "$0.25"); // <1 → 两位小数
  assert.equal(fmtUsd(400, 200), "$2.0"); // ≥1 <100 → 一位小数
  assert.equal(fmtUsd(400, 0), "$2.0"); // rate<=0 回落 200
});

test("fmtAmount：credits 原值缩写", () => {
  assert.equal(fmtAmount(878), "878");
  assert.equal(fmtAmount(39200), "39.2k");
  assert.equal(fmtAmount(2970), "3.0k");
  assert.equal(fmtAmount(0), "0");
});

test("deriveWindow 保留原始额度 used/budget", () => {
  const w = deriveWindow({ name: "7d", used: 32700, budget: 74600, reset_at: 1_500_000 }, 1_000_000);
  assert.equal(w.used, 32700);
  assert.equal(w.budget, 74600);
});

test("deriveAll 显示 relay 返回的所有窗口（含未预置的 7d_fable）", () => {
  const now = 1_000_000;
  const all = deriveAll(
    {
      windows: [
        { name: "7d_fable", used: 3167, budget: 74200, reset_at: now + 600000 },
        { name: "5h", used: 100, budget: 1000, reset_at: now + 3600 },
        { name: "7d", used: 700, budget: 1000, reset_at: now + 600000 },
      ],
    },
    now,
  );
  // 全部保留，按时长升序（5h < 7d = 7d_fable，同长按名字）
  assert.deepEqual(all.windows.map((w) => w.name), ["5h", "7d", "7d_fable"]);
  const fable = all.windows.find((w) => w.name === "7d_fable");
  assert.equal(fable.label, "7 天窗口 · fable");
  assert.equal(fable.budget, 74200);
  assert.ok(fable.pacePct != null, "7d 时长可从名字解析 → 有匀速线");
});

test("deriveWindow 未知窗口名也显示，但无匀速线", () => {
  const w = deriveWindow({ name: "weird", used: 50, budget: 100, reset_at: 2_000_000 }, 1_000_000);
  assert.ok(w);
  assert.equal(w.usedPct, 50);
  assert.equal(w.label, "weird");
  assert.equal(w.pacePct, null);
  assert.equal(w.deltaText, null);
});

test("limitsMatchAccount：subject 归属判定（切号后过滤旧账号用量）", () => {
  // subject 与当前 userId 一致 → 属于当前账号
  assert.equal(limitsMatchAccount({ subject: "usr_a", windows: [] }, "usr_a"), true);
  // subject 属于别的账号（切号过渡期 relay 还没追上）→ 不匹配
  assert.equal(limitsMatchAccount({ subject: "usr_a", windows: [] }, "usr_b"), false);
  // 未登录 / 拿不到当前 userId → 不阻拦
  assert.equal(limitsMatchAccount({ subject: "usr_a" }, null), true);
  assert.equal(limitsMatchAccount({ subject: "usr_a" }, undefined), true);
  // 响应无 subject（老 relay）→ 向后兼容，不阻拦
  assert.equal(limitsMatchAccount({ windows: [] }, "usr_a"), true);
  assert.equal(limitsMatchAccount(null, "usr_a"), true);
});

test("用户截图数值复现：5h 卡（匀速线 23%，落后 20%）", () => {
  // 截图：已用 3%，"3 小时后重置"（实际 3h51m 余量 → 已过 1h09m）
  const now = 1_000_000;
  const w = { name: "5h", used: 3, budget: 100, reset_at: now + 3 * 3600 + 51 * 60 };
  const d = deriveWindow(w, now);
  assert.equal(d.usedPct, 3);
  assert.equal(d.pacePct, 23);
  assert.equal(d.delta, -20);
  assert.match(d.deltaText, /落后 20%/);
});

test("匀速线夹在 [0,100]，reset_at 已过期不产生负数", () => {
  const now = 2_000_000;
  const d = deriveWindow({ name: "5h", used: 50, budget: 100, reset_at: now - 10 }, now);
  assert.equal(d.pacePct, 100);
  const d2 = deriveWindow({ name: "5h", used: 0, budget: 100, reset_at: now + 18000 + 999 }, now);
  assert.equal(d2.pacePct, 0); // 剩余超过窗口长度也不为负
});

test("超前/落后符号与文案", () => {
  const now = 0;
  // 用得比匀速快 → 超前
  const ahead = deriveWindow({ name: "7d", used: 50, budget: 100, reset_at: now + 604800 * 0.9 }, now);
  assert.ok(ahead.delta > 0);
  assert.match(ahead.deltaText, /超前/);
  // delta = 0 边界归入"超前"
  const even = deriveWindow({ name: "7d", used: 10, budget: 100, reset_at: now + 604800 * 0.9 }, now);
  assert.equal(even.delta, 0);
  assert.match(even.deltaText, /超前 0%/);
});

test("倒计时文案三档边界", () => {
  assert.equal(resetText(29 * 60), "29 分后重置");
  assert.equal(resetText(3600), "1 小时 0 分后重置");
  assert.equal(resetText(86400 - 1), "23 小时 59 分后重置");
  assert.equal(resetText(86400), "1 天 0 小时后重置");
  assert.equal(resetText(6 * 86400 + 3600 * 5), "6 天 5 小时后重置");
  assert.equal(resetText(-5), "0 分后重置");
});

test("真实夹具：三窗口齐、排序固定、最紧窗口正确", () => {
  const now = Math.min(...fixture.windows.map((w) => w.reset_at)) - 60;
  const all = deriveAll(fixture, now);
  assert.equal(all.windows.length, 3);
  assert.deepEqual(all.windows.map((w) => w.name), ["5h", "7d", "30d"]);
  const maxUsed = Math.max(...all.windows.map((w) => w.usedPct));
  assert.equal(all.tight.usedPct, maxUsed);
  assert.equal(all.status.kind, "ok");
});

test("状态优先级：suspended > degraded > unmetered", () => {
  assert.equal(deriveStatus({ suspended: true, degraded: true, unmetered: true }).dot, "red");
  assert.equal(deriveStatus({ degraded: true, unmetered: true }).dot, "amber");
  assert.equal(deriveStatus({ unmetered: true }).dot, "blue");
  assert.equal(deriveStatus({}).kind, "ok");
});

test("坏数据不炸：budget<=0 丢弃；无法解析时长的窗口名保留（无匀速线）", () => {
  const all = deriveAll(
    { windows: [{ name: "5h", used: 1, budget: 0, reset_at: 10 }, { name: "1y", used: 1, budget: 5, reset_at: 10 }] },
    0,
  );
  assert.equal(all.windows.length, 1); // budget<=0 的 5h 丢弃；"1y" 保留
  assert.equal(all.windows[0].name, "1y");
  assert.equal(all.windows[0].pacePct, null); // "y" 非 m/h/d/w → 解析不出时长 → 无匀速线
});
