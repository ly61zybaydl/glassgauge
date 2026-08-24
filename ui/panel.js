// 渲染与数据环。派生计算全部来自 derive.js；本文件只做取数节奏和 DOM。
import { accountItems, currentLabel, esc, planBadge, planExpiry } from "./accounts-view.js";
import { deriveAll, limitsMatchAccount, fmtUsd, fmtAmount } from "./derive.js";
import { initGlass, recropTo, reloadWallpaper, teardownGlass } from "./glass.js";
import { applyWallpaperTheme } from "./theme.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

let config = null;
let lastGood = null; // 最后一次成功的 {json, at}
let timer = null;
let backoffMs = 0; // 0 = 正常节奏；失败后 5s→30s
let lastConnected = true;
let expanded = false; // 悬停展开态（spec 形态 C）
let fastUntil = 0; // 切号后允许快轮询到此时刻（等 relay 的 /v1/limits 追上新账号）
let lastTickAt = 0; // 上次进入 tick 的时刻（看门狗判活）

// 玻璃设置（⚙ 弹出行）：白纱 alpha 即时生效（CSS 变量），磨砂 blur 防抖后
// 经 set_glass 持久化并热更引擎/壁纸滤镜
let setOpen = false;
let glassCommitTimer = null;

// 账号切换（读写 ~/.mirasim，与 relay 无关——断连时也可用，正是需要切号的场景）
let accounts = null; // 最后一次 accounts_list 的视图（只含元数据，无令牌）
let acctOpen = false; // 快照列表展开
let acctBusy = false; // 切换/保存进行中，屏蔽点击
let acctNote = null; // 瞬时提示 {text, kind:"ok"|"err"}
let acctNoteTimer = null;
let acctConfirm = null; // 待二次确认删除的快照名

// 生效玻璃模式（spec §4）：refract=原生折射 | wallpaper=壁纸折射兜底 | live=DWM 亚克力。
// 启动时问 get_glass_mode，之后由 glass-mode 事件驱动（引擎降级/恢复）。
let glassMode = "refract";

function applyRadius() {
  const g = config?.glass ?? {};
  // live 模式 DWM 只能裁 ~8px 圆角，CSS 必须一致；refract/wallpaper 都是 20px 药丸
  document.documentElement.style.setProperty(
    "--radius-collapsed",
    glassMode === "live" ? "8px" : (g.radiusCollapsed ?? 20) + "px",
  );
}

// 白纱够浓（面板接近纯白）时切到"实底"配色：文字/刻度转深色，卡片加淡描边，
// 否则深色壁纸下会白字压白底看不清。挂 root 上，拖动滑杆即时生效、跨重渲染保留。
function applySolid() {
  const a = config?.glass?.alpha ?? 0.03;
  document.documentElement.classList.toggle("solid", a >= 0.5);
}

async function loadConfig() {
  config = JSON.parse(await invoke("get_config"));
  const g = config.glass ?? {};
  const root = document.documentElement.style;
  if (g.alpha != null) root.setProperty("--alpha", g.alpha);
  if (g.radiusCard != null) root.setProperty("--radius-card", g.radiusCard + "px");
  applyRadius();
  applySolid();
}

/* ---------- 取数节奏：正常 refreshSeconds 轮询；失败快重试（≤5s）------------
   整个函数体包在 try/finally 里：无论取数/渲染是否抛错，finally 永远重排下一次 tick，
   保证轮询链不会因一次异常而停摆（否则开机时序等边角会让挂件永久卡在"不可用"）。 */
async function tick() {
  clearTimeout(timer);
  lastTickAt = Date.now();
  let ok = false;
  try {
    try {
      const res = await invoke("fetch_limits");
      lastGood = { json: JSON.parse(res.json), at: Date.now() };
      ok = true;
    } catch {
      /* relay-not-found 或网络失败 → 降级渲染 */
    }
    try {
      accounts = await invoke("accounts_list"); // 本地文件读，与 relay 独立
    } catch {
      /* setting.json 缺失/损坏 → 沿用上次视图（可能为 null，不画账号行） */
    }
    lastConnected = ok;
    render(ok);
    // expand:"always"（默认）= 常驻展开：首次取数后即展开定形——
    // 即便 relay 没找到也展开，账号切换要在这种时候可用
    if ((config?.expand ?? "always") === "always" && !expanded) {
      setExpanded(true);
    }
  } catch (e) {
    console.error("tick error:", e);
  } finally {
    let delay;
    if (ok) {
      backoffMs = 0;
      // 切号后 relay 的 limits 还属于旧账号 → 快轮询（3s）直到 subject 追上或超时
      delay = limitsSyncing() && Date.now() < fastUntil ? 3000 : (config?.refreshSeconds ?? 60) * 1000;
    } else {
      // 等 mirasim relay（开机/重启时序）→ 快重试，最多 5s，好让它一起来就秒级恢复
      backoffMs = backoffMs ? Math.min(backoffMs * 2, 5000) : 2000;
      delay = backoffMs;
    }
    timer = setTimeout(tick, delay);
  }
}

// 看门狗：万一定时器链意外中断（未捕获异常等），兜底把 tick 重新拉起来
setInterval(() => {
  if (Date.now() - lastTickAt > 90000) tick();
}, 30000);

/* 只认属于当前账号的用量：切号后 relay 的 /v1/limits 会有个把秒仍报旧账号，
   subject≠当前 userId 期间把旧数据视作"暂无"，避免显示上一个账号的用量。 */
function currentLimits() {
  if (!lastGood) return null;
  return limitsMatchAccount(lastGood.json, accounts?.current?.userId) ? lastGood.json : null;
}
// 有数据但 subject 属于别的账号 = 正在等 relay 追上新账号
function limitsSyncing() {
  return !!lastGood && !limitsMatchAccount(lastGood.json, accounts?.current?.userId);
}

/* ---------- 渲染 ---------- */
function render(connected) {
  const app = document.getElementById("app");
  const limits = currentLimits();
  const syncing = limitsSyncing();
  if (expanded) {
    // 无（当前账号的）数据也渲染展开壳（空态卡位 + 账号管理可用）
    const all = limits ? deriveAll(limits, Date.now() / 1000) : null;
    app.innerHTML = expandedHtml(all, connected, syncing);
    markDragRegion();
    fitExpanded();
    return;
  }
  if (!limits) {
    app.innerHTML = shellHtml({
      dot: "grey",
      who: !connected ? "等待 Mirasim 启动…" : syncing ? "同步新账号用量…" : "加载中…",
      pct: "–",
      fill: 0,
      tickAt: 0,
      stale: !connected,
    });
    markDragRegion();
    return;
  }
  const all = deriveAll(limits, Date.now() / 1000);
  const t = all.tight;
  const abnormal = all.status.kind !== "ok";
  app.innerHTML = shellHtml({
    dot: connected ? all.status.dot : "grey",
    who: abnormal
      ? all.status.text
      : connected
        ? `最紧窗口 · ${t ? shortName(t.name) : "–"}`
        : "连接丢失 · 显示最后数据",
    pct: t ? t.usedPct + "%" : "–",
    fill: t ? t.usedPct : 0,
    tickAt: t && t.pacePct != null ? t.pacePct : 0,
    stale: !connected,
  });
  markDragRegion();
}

/* ---------- 展开态（spec 形态 C：304 宽，三窗口卡 + 账号行） ---------- */
function expandedHtml(all, connected, syncing) {
  const dot = connected ? (all?.status.dot ?? "grey") : "grey";
  const dotCls = dot === "accent" ? "dot" : `dot ${dot}`;
  const emptyMsg = !connected
    ? "等待 Mirasim 启动 · 自动重连中…"
    : syncing
      ? "正在同步新账号用量…"
      : "加载中…";
  // 费用折算：普通窗口 unitsPerUsd（默认 200 units=$1），Fable 窗口贵 fableMultiplier
  // （默认 2.4）倍 → 480 units=$1。名字含 fable 的窗口按 Fable 单价。
  const base = config?.unitsPerUsd ?? 200;
  const fableMult = config?.fableMultiplier ?? 2.4;
  const cards = all
    ? all.windows
        .map((w) => {
          const rate = /fable/i.test(w.name) ? base * fableMult : base;
          return `
      <div class="card">
        <div class="r1">
          <span class="win">${w.label}</span>
          <span class="pct2">${w.usedPct}%</span>
        </div>
        <div class="amt">${fmtAmount(w.used)} / ${fmtAmount(w.budget)} · ≈ ${fmtUsd(w.used, rate)} / ${fmtUsd(w.budget, rate)}</div>
        <div class="bar">
          <div class="fill" style="width:${w.usedPct}%"></div>
          ${w.pacePct != null ? `<div class="tick" style="left:${w.pacePct}%"></div>` : ""}
        </div>
        <div class="l3"><span>${w.resetText}</span><span class="d">${w.deltaText ?? ""}</span></div>
      </div>`;
        })
        .join("")
    : `<div class="card empty">${emptyMsg}</div>`;
  return `
    <div class="shell expanded${connected ? "" : " stale"}">
      <div class="head">
        <span class="${dotCls}"></span>
        <span class="title">Mirasim 用量</span>
        <span class="badge">${esc(planBadge(accounts, config?.planLabel ?? "MAX"))}</span>
        <span class="exp">套餐到期 ${esc(planExpiry(accounts, config?.validUntil ?? "–"))}</span>
        <span class="gear${setOpen ? " on" : ""}" data-gg-interactive data-set-act="toggle">⚙</span>
      </div>
      ${setHtml()}
      ${acctHtml()}
      ${cards}
    </div>`;
}

/* ---------- 玻璃设置（⚙）：白纱 = 透出多少桌面，磨砂 = 模糊多少 ---------- */
function setHtml() {
  if (!setOpen) return "";
  const g = config?.glass ?? {};
  const alpha = g.alpha ?? 0.03;
  const blur = g.blur ?? 4;
  // live 模式的模糊由 DWM 亚克力固定，只给白纱；refract/wallpaper 两条都给
  const blurRow =
    glassMode === "live"
      ? ""
      : `
      <div class="set-row">
        <span class="set-k">磨砂</span>
        <input type="range" min="0" max="14" step="0.5" value="${blur}" data-set-glass="blur">
        <span class="set-v">${blur}</span>
      </div>`;
  return `
    <div class="set" data-gg-interactive>
      <div class="set-row">
        <span class="set-k">白纱</span>
        <input type="range" min="0" max="1" step="0.02" value="${alpha}" data-set-glass="alpha">
        <span class="set-v">${Math.round(alpha * 100)}%</span>
      </div>
      ${blurRow}
      <div class="set-hint">白纱 0% = 纯玻璃全透，100% = 纯白面板 · 拖动即时生效并记住</div>
    </div>`;
}

function onGlassInput(input) {
  const key = input.getAttribute("data-set-glass");
  const val = Number(input.value);
  config.glass = config.glass ?? {};
  config.glass[key] = val;
  // 白纱即时走 CSS 变量；数值角标就地改，不整树重渲染（拖动中会打断滑杆）
  if (key === "alpha") {
    document.documentElement.style.setProperty("--alpha", val);
    applySolid();
    const v = input.parentElement.querySelector(".set-v");
    if (v) v.textContent = Math.round(val * 100) + "%";
  } else {
    const v = input.parentElement.querySelector(".set-v");
    if (v) v.textContent = String(val);
  }
  // 防抖提交：持久化 + refract 引擎热更；wallpaper 模式重建本地滤镜
  clearTimeout(glassCommitTimer);
  glassCommitTimer = setTimeout(async () => {
    try {
      await invoke("set_glass", {
        alpha: config.glass.alpha ?? null,
        blur: config.glass.blur ?? null,
      });
      if (glassMode === "wallpaper") initGlass(config).catch(() => {});
    } catch (e) {
      console.error("set_glass:", e);
    }
  }, 350);
}

/* ---------- 账号切换（数据来自 accounts_list，只有元数据没有令牌） ---------- */
function acctHtml() {
  if (!accounts) return ""; // 还没读到 setting.json：不画账号行
  const label = acctBusy ? "切换中…" : currentLabel(accounts);
  const row = `
      <div class="acct-row${acctBusy ? " busy" : ""}" data-acct-act="toggle">
        <span class="acct-k">账号</span>
        <span class="acct-name">${esc(label)}</span>
        <span class="acct-caret${acctOpen ? " open" : ""}">▾</span>
      </div>`;
  let list = "";
  if (acctOpen && !acctBusy) {
    const items = accountItems(accounts, Date.now())
      .map((it) => {
        const del = it.current
          ? ""
          : `<span class="acct-del${acctConfirm === it.name ? " arm" : ""}" data-acct-del="${esc(it.name)}">${
              acctConfirm === it.name ? "确认删除" : "✕"
            }</span>`;
        return `
        <div class="acct-item${it.current ? " cur" : ""}" data-acct-act="switch" data-name="${esc(it.name)}">
          <span class="acct-dot${it.current ? " on" : ""}"></span>
          <span class="acct-item-name">${esc(it.name)}</span>
          <span class="acct-item-sub">${esc(it.sub)}</span>
          ${del}
        </div>`;
      })
      .join("");
    const hint = accounts.profiles.length
      ? ""
      : `<div class="acct-hint">还没有快照：登录后点下面保存；换账号登录再存一个，即可来回切换</div>`;
    const save = accounts.current
      ? `<div class="acct-item save" data-acct-act="save">＋ 保存当前登录为快照</div>`
      : `<div class="acct-hint">当前未登录 · 点任意快照可直接恢复该账号</div>`;
    list = `<div class="acct-list">${hint}${items}${save}</div>`;
  }
  const note = acctNote
    ? `<div class="acct-note ${acctNote.kind}">${esc(acctNote.text)}</div>`
    : "";
  return `<div class="acct" data-gg-interactive>${row}${list}${note}</div>`;
}

function setNote(text, kind) {
  clearTimeout(acctNoteTimer);
  acctNote = text ? { text, kind } : null;
  if (text) {
    acctNoteTimer = setTimeout(() => {
      acctNote = null;
      render(lastConnected);
    }, 5000);
  }
}

async function doSwitch(name) {
  acctBusy = true;
  setNote(null);
  render(lastConnected);
  try {
    accounts = await invoke("accounts_switch", { name });
    setNote(`已切换 →「${name}」· 同步用量中…`, "ok");
    // mirasim 服务端热重载 setting.json 后，/v1/limits 的 subject 才会换到新账号；
    // 开一个快轮询窗口（最多 150s），tick 里按 subject 是否追上决定快/慢节奏
    fastUntil = Date.now() + 150000;
    tick();
  } catch (e) {
    setNote(String(e), "err");
  }
  acctBusy = false;
  acctOpen = false;
  acctConfirm = null;
  render(lastConnected);
}

async function doSave() {
  acctBusy = true;
  render(lastConnected);
  try {
    accounts = await invoke("accounts_save", { name: null });
    setNote(`已保存快照「${accounts.current?.profile ?? "?"}」`, "ok");
  } catch (e) {
    setNote(String(e), "err");
  }
  acctBusy = false;
  render(lastConnected);
}

async function doRemove(name) {
  if (acctConfirm !== name) {
    // 两步确认：第一次点 ✕ 变"确认删除"，3 秒不点自动还原
    acctConfirm = name;
    render(lastConnected);
    setTimeout(() => {
      if (acctConfirm === name) {
        acctConfirm = null;
        render(lastConnected);
      }
    }, 3000);
    return;
  }
  acctConfirm = null;
  try {
    accounts = await invoke("accounts_remove", { name });
    setNote(`已删除快照「${name}」`, "ok");
  } catch (e) {
    setNote(String(e), "err");
  }
  render(lastConnected);
}

document.getElementById("app").addEventListener("input", (e) => {
  const input = e.target.closest("[data-set-glass]");
  if (input) onGlassInput(input);
});

document.getElementById("app").addEventListener("click", (e) => {
  const gear = e.target.closest("[data-set-act]");
  if (gear) {
    setOpen = !setOpen;
    render(lastConnected);
    return;
  }
  if (acctBusy) return;
  const del = e.target.closest("[data-acct-del]");
  if (del) {
    doRemove(del.getAttribute("data-acct-del"));
    return;
  }
  const el = e.target.closest("[data-acct-act]");
  if (!el) return;
  const act = el.getAttribute("data-acct-act");
  if (act === "toggle") {
    acctOpen = !acctOpen;
    acctConfirm = null;
    render(lastConnected);
  } else if (act === "save") {
    doSave();
  } else if (act === "switch") {
    const name = el.getAttribute("data-name");
    const cur = accounts?.profiles.find((p) => p.name === name)?.current;
    if (cur) {
      acctOpen = false; // 点当前账号 = 收起
      render(lastConnected);
    } else {
      doSwitch(name);
    }
  }
});

const COLLAPSED_SIZE = [244, 62];
const EXPANDED_W = 304;

// 展开壳定宽 302，渲染后量自然高，窗口跟内容走（账号列表开合也会变高）
let lastFitH = 0;
async function fitExpanded() {
  const shell = document.querySelector(".shell.expanded");
  if (!shell) return;
  const h = shell.offsetHeight + 2;
  if (h === lastFitH) return;
  lastFitH = h;
  const { LogicalSize } = window.__TAURI__.dpi;
  await appWindow.setSize(new LogicalSize(EXPANDED_W, h));
  // 壁纸兜底模式的滤镜/裁剪是按窗口尺寸建的，尺寸变了要重建
  if (glassMode === "wallpaper") {
    setTimeout(() => initGlass(config).catch(() => {}), 120);
  }
}

async function setExpanded(v) {
  if (expanded === v) return;
  expanded = v;
  if (v) {
    render(lastConnected); // render 内部 fitExpanded 负责量高调窗口
    return;
  }
  lastFitH = 0;
  acctOpen = false;
  render(lastConnected);
  const { LogicalSize } = window.__TAURI__.dpi;
  await appWindow.setSize(new LogicalSize(COLLAPSED_SIZE[0], COLLAPSED_SIZE[1]));
  if (glassMode === "wallpaper") {
    setTimeout(() => initGlass(config).catch(() => {}), 120);
  }
}

// expand:"hover" 才启用悬停展开/收起；"always" 常驻展开不理会指针
let hoverTimer = null;
document.addEventListener("pointerenter", () => {
  if ((config?.expand ?? "always") === "always") return;
  clearTimeout(hoverTimer);
  hoverTimer = setTimeout(() => setExpanded(true), 120);
});
document.addEventListener("pointerleave", () => {
  if ((config?.expand ?? "always") === "always") return;
  clearTimeout(hoverTimer);
  hoverTimer = setTimeout(() => setExpanded(false), 280);
});

function shellHtml({ dot, who, pct, fill, tickAt, stale }) {
  const dotCls = dot === "accent" ? "dot" : `dot ${dot}`;
  return `
    <div class="shell${stale ? " stale" : ""}">
      <div class="top">
        <span class="${dotCls}"></span>
        <span class="who">${who}</span>
        <span class="pct">${pct}</span>
      </div>
      <div class="bar">
        <div class="fill" style="width:${fill}%"></div>
        <div class="tick" style="left:${tickAt}%"></div>
      </div>
    </div>`;
}

function shortName(n) {
  return { "5h": "5 小时", "7d": "7 天", "30d": "30 天" }[n] ?? n;
}

/* ---------- 拖动与位置 ---------- */
function markDragRegion() {
  for (const el of document.querySelectorAll("body, #app, #app *")) {
    if (!el.closest("[data-gg-interactive]")) {
      el.setAttribute("data-tauri-drag-region", "");
    }
  }
}

let moveTimer = null;
appWindow.onMoved(({ payload }) => {
  recropTo(payload.x, payload.y); // 玻璃跟手：拖动中每次事件都重摆背景
  clearTimeout(moveTimer);
  moveTimer = setTimeout(() => {
    invoke("save_state", { x: payload.x, y: payload.y });
  }, 500);
});

/* ---------- 启动 ---------- */
(async () => {
  await loadConfig();
  glassMode = await invoke("get_glass_mode").catch(() => "wallpaper");
  applyRadius();
  render(true); // 先画"加载中"
  applyWallpaperTheme(config); // 主色/明暗跟壁纸走（与玻璃模式无关）
  // 壁纸折射层只在 wallpaper 模式（含引擎降级）启用；失败只降级为素壳，不挡数据
  if (glassMode === "wallpaper") initGlass(config).catch((e) => console.error("glass init:", e));
  await listen("glass-mode", ({ payload }) => {
    glassMode = payload;
    applyRadius();
    if (payload === "wallpaper") initGlass(config).catch(() => {});
    else teardownGlass();
  });
  await listen("manual-refresh", () => {
    // 托盘刷新 = 配置热载 + 重取色 + 重读壁纸 + 立即拉数
    loadConfig().then(() => {
      applyWallpaperTheme(config);
      tick();
    });
    if (glassMode === "wallpaper") reloadWallpaper().catch(() => {});
  });
  await listen("wallpaper-changed", () => {
    applyWallpaperTheme(config);
    if (glassMode === "wallpaper") reloadWallpaper().catch(() => {});
  });
  tick();
})();
