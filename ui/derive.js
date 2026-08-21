// 用量派生计算（spec §4.3）。纯函数，node --test 可测，不碰 DOM。

export const WINDOW_LEN = { "5h": 18000, "7d": 604800, "30d": 2592000 };
export const WINDOW_LABEL = { "5h": "5 小时窗口", "7d": "7 天窗口", "30d": "30 天窗口" };

/** 单个窗口 -> 显示值。now 为 Unix 秒。 */
export function deriveWindow(w, now) {
  const len = WINDOW_LEN[w.name];
  if (!len || !(w.budget > 0)) return null;
  const usedPct = (w.used / w.budget) * 100;
  const remaining = Math.max(0, w.reset_at - now);
  const pacePct = Math.min(100, Math.max(0, ((len - remaining) / len) * 100));
  const delta = usedPct - pacePct;
  return {
    name: w.name,
    label: WINDOW_LABEL[w.name] ?? w.name,
    usedPct: round1(usedPct),
    remPct: Math.max(0, Math.round(100 - usedPct)),
    pacePct: round1(pacePct),
    delta: round1(delta),
    deltaText: `匀速线 ${round1(pacePct)}% · ${delta >= 0 ? "超前" : "落后"} ${Math.abs(round1(delta))}%`,
    resetText: resetText(remaining),
    used: w.used,
    budget: w.budget,
  };
}

/** 额度 → 估算 API 费用（美元）。perUsd = 多少 credits 折合 $1（默认 100）。
 *  ≥1000 用 k、≥100 取整、≥1 一位小数、否则两位小数，前缀 $。
 *  注意：费率是估算（非 mirasim 官方口径），仅供参考。 */
export function fmtUsd(credits, perUsd) {
  const rate = perUsd > 0 ? perUsd : 100;
  const usd = (Number(credits) || 0) / rate;
  let s;
  if (usd >= 1000) s = (usd / 1000).toFixed(1) + "k";
  else if (usd >= 100) s = String(Math.round(usd));
  else if (usd >= 1) s = usd.toFixed(1);
  else s = usd.toFixed(2);
  return "$" + s;
}

/** credits 原值缩写：<1000 取整；≥1000 用一位小数 k（39200 → "39.2k"，878 → "878"）。 */
export function fmtAmount(n) {
  const v = Number(n) || 0;
  return v < 1000 ? String(Math.round(v)) : (v / 1000).toFixed(1) + "k";
}

/** 全响应 -> {status, windows[], tight}。窗口按 5h/7d/30d 固定排序。 */
export function deriveAll(limits, now) {
  const order = ["5h", "7d", "30d"];
  const windows = (limits.windows ?? [])
    .map((w) => deriveWindow(w, now))
    .filter(Boolean)
    .sort((a, b) => order.indexOf(a.name) - order.indexOf(b.name));
  return {
    status: deriveStatus(limits),
    windows,
    tight: tightest(windows),
  };
}

/** 最紧窗口 = 已用百分比最大者；空数组返回 null。 */
export function tightest(derivedWindows) {
  return derivedWindows.reduce((a, b) => (a == null || b.usedPct > a.usedPct ? b : a), null);
}

/** suspended/degraded/unmetered -> 状态点（spec §7）。优先级：红 > 黄 > 蓝 > 正常。 */
export function deriveStatus(limits) {
  if (limits.suspended) return { kind: "suspended", dot: "red", text: "账号已暂停" };
  if (limits.degraded) return { kind: "degraded", dot: "amber", text: "服务降级中" };
  if (limits.unmetered) return { kind: "unmetered", dot: "blue", text: "不计量模式" };
  return { kind: "ok", dot: "accent", text: null };
}

/** 剩余秒 -> 倒计时文案。>=1 天给 天+小时；>=1 小时给 小时+分；否则只给分。 */
export function resetText(sec) {
  const s = Math.max(0, Math.floor(sec));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 小时后重置`;
  if (h > 0) return `${h} 小时 ${m} 分后重置`;
  return `${m} 分后重置`;
}

/** 用量是否属于该账号：`/v1/limits` 的 subject 应等于当前登录 userId。
 *  无 userId（未登录/未取到）或响应无 subject 时不阻拦（视作匹配，向后兼容）。
 *  切号后 relay 有个把秒仍在报旧账号，此时 subject≠userId → 返回 false，
 *  上层据此把旧账号用量视作"暂无"，不显示上一个账号的数字。 */
export function limitsMatchAccount(limits, userId) {
  const subject = limits?.subject;
  if (!userId || !subject) return true;
  return subject === userId;
}

function round1(x) {
  return Math.round(x * 10) / 10;
}
