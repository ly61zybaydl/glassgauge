// 账号切换的视图模型：排序/文案，纯函数，node --test 可测，不碰 DOM。

/** 账号行主文案：邮箱优先，回退账号显示名 / userId / 未登录。 */
export function currentLabel(view) {
  if (!view?.current) return "未登录";
  const c = view.current;
  return c.email || c.name || c.userId || "未知账号";
}

/** 下拉列表项：当前账号置顶，其余按快照名 zh 排序。子行邮箱优先，回退账号名。 */
export function accountItems(view, nowMs) {
  const items = (view?.profiles ?? []).map((p) => ({
    name: p.name,
    current: !!p.current,
    sub: `${p.email || p.accountName || "?"} · ${savedAgo(p.savedAt, nowMs)}`,
  }));
  items.sort((a, b) => (b.current - a.current) || a.name.localeCompare(b.name, "zh"));
  return items;
}

/** 快照保存时间 -> 相对文案。 */
export function savedAgo(ms, nowMs) {
  if (!ms) return "时间未知";
  const s = Math.max(0, Math.floor((nowMs - ms) / 1000));
  if (s < 60) return "刚保存";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** HTML 转义（文本与双引号属性通用）。快照名/账号名是用户可控字符串，必须过这里。 */
export function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]
  ));
}
