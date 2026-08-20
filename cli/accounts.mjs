#!/usr/bin/env node
/**
 * mirasim 订阅账号切换器
 *
 * 管理 ~/.mirasim/setting.json 里的 auth 登录态（token/refreshToken 为 mrs1: 密文，
 * 由同目录 secret.key 在本机加解密，因此快照只在本机有效）。
 * 快照保存在 <home>/_account_switcher/profiles/，切换前自动：
 *   1. 把当前登录态回存到对应快照（保持 refreshToken 最新）
 *   2. 备份整个 setting.json 到 <home>/_account_switcher/backups/
 *   3. 原子写入新的 auth
 *
 * 用法见 README.md 或运行 `node accounts.mjs help`。
 */

import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import readline from 'node:readline/promises';

// ---------- 参数解析 ----------

const rawArgs = process.argv.slice(2);
const flags = { home: null, force: false, yes: false };
const positional = [];
for (let i = 0; i < rawArgs.length; i++) {
  const a = rawArgs[i];
  if (a === '--home') flags.home = rawArgs[++i];
  else if (a.startsWith('--home=')) flags.home = a.slice(7);
  else if (a === '--force' || a === '-f') flags.force = true;
  else if (a === '--yes' || a === '-y') flags.yes = true;
  else if (a === '--help' || a === '-h') positional.unshift('help');
  else positional.push(a);
}

const HOME = path.resolve(
  flags.home || process.env.MIRASIM_HOME?.trim() || path.join(os.homedir(), '.mirasim')
);
const SETTING = path.join(HOME, 'setting.json');
const TOOL_DIR = path.join(HOME, '_account_switcher');
const PROFILE_DIR = path.join(TOOL_DIR, 'profiles');
const BACKUP_DIR = path.join(TOOL_DIR, 'backups');
const BACKUP_KEEP = 20;

// ---------- 基础工具 ----------

function fail(msg) {
  console.error('✖ ' + msg);
  process.exit(1);
}

function readJson(file) {
  const text = fs.readFileSync(file, 'utf8').replace(/^﻿/, '');
  return JSON.parse(text);
}

function writeJsonAtomic(file, obj) {
  const tmp = file + '.tmp-' + process.pid;
  fs.writeFileSync(tmp, JSON.stringify(obj, null, 2), 'utf8');
  fs.renameSync(tmp, file); // Windows 下 rename 覆盖同卷已有文件，等效原子替换
}

function loadSetting() {
  if (!fs.existsSync(SETTING)) fail(`找不到 ${SETTING}（mirasim 数据目录不对？可用 --home 指定）`);
  try {
    return readJson(SETTING);
  } catch (e) {
    fail(`setting.json 解析失败：${e.message}`);
  }
}

function hasLogin(auth) {
  return !!(auth && typeof auth === 'object' && auth.token && auth.userId);
}

function shortId(userId) {
  return String(userId || '').replace(/^usr_/, '').slice(0, 8) || '????????';
}

function sanitizeName(name) {
  const s = String(name || '').trim().replace(/[^\w一-鿿.-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '');
  return s.slice(0, 40);
}

function fmtTime(ms) {
  if (!ms) return '-';
  const d = new Date(ms);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

function expInfo(auth) {
  if (!auth?.exp) return '';
  const expMs = auth.exp * 1000;
  return expMs > Date.now() ? `令牌有效至 ${fmtTime(expMs)}` : '访问令牌已过期（应用启动后会用 refreshToken 自动续期）';
}

// ---------- 快照存取 ----------

function ensureDirs() {
  fs.mkdirSync(PROFILE_DIR, { recursive: true });
  fs.mkdirSync(BACKUP_DIR, { recursive: true });
}

function listProfiles() {
  if (!fs.existsSync(PROFILE_DIR)) return [];
  const out = [];
  for (const f of fs.readdirSync(PROFILE_DIR)) {
    if (!f.endsWith('.json')) continue;
    try {
      const p = readJson(path.join(PROFILE_DIR, f));
      if (hasLogin(p.auth)) out.push({ ...p, file: path.join(PROFILE_DIR, f) });
    } catch {
      console.error(`（警告：快照 ${f} 损坏，已跳过）`);
    }
  }
  out.sort((a, b) => String(a.name).localeCompare(String(b.name), 'zh'));
  return out;
}

function profilePath(name) {
  return path.join(PROFILE_DIR, name + '.json');
}

function saveProfile(name, auth, extra = {}) {
  ensureDirs();
  const record = {
    name,
    userId: auth.userId,
    accountName: auth.name || '',
    savedAt: Date.now(),
    ...extra,
    auth,
  };
  writeJsonAtomic(profilePath(name), record);
  return record;
}

/** 把当前 setting.json 的登录态回存到同 userId 的快照；没有则自动新建。返回描述文字。 */
function snapshotCurrent(setting, profiles) {
  const auth = setting.auth;
  if (!hasLogin(auth)) return null;
  const hit = profiles.find((p) => p.userId === auth.userId);
  if (hit) {
    saveProfile(hit.name, auth);
    return `已回存当前账号最新登录态 → 快照「${hit.name}」`;
  }
  let name = sanitizeName(auth.name) || 'usr-' + shortId(auth.userId);
  while (fs.existsSync(profilePath(name))) name += '-' + shortId(auth.userId).slice(0, 4);
  saveProfile(name, auth);
  return `当前账号还没有快照，已自动保存为「${name}」（避免切换后丢失登录态）`;
}

function backupSetting() {
  ensureDirs();
  const p = (n) => String(n).padStart(2, '0');
  const d = new Date();
  const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  const dest = path.join(BACKUP_DIR, `setting-${stamp}.json`);
  fs.copyFileSync(SETTING, dest);
  const all = fs.readdirSync(BACKUP_DIR).filter((f) => f.startsWith('setting-')).sort();
  for (const f of all.slice(0, Math.max(0, all.length - BACKUP_KEEP))) {
    fs.rmSync(path.join(BACKUP_DIR, f));
  }
  return dest;
}

// ---------- 运行中检测 ----------

function mirasimRunning() {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync('tasklist', ['/FI', 'IMAGENAME eq Mirasim.exe', '/FO', 'CSV', '/NH'], {
        encoding: 'utf8', windowsHide: true,
      });
      return /Mirasim\.exe/i.test(out);
    }
    execFileSync('pgrep', ['-if', 'mirasim'], { encoding: 'utf8' });
    return true;
  } catch {
    return false;
  }
}

async function confirm(question) {
  if (flags.yes || flags.force) return true;
  if (!process.stdin.isTTY) {
    console.error('（非交互终端，默认取消；加 --yes 跳过确认）');
    return false;
  }
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const ans = (await rl.question(question + ' [y/N] ')).trim().toLowerCase();
  rl.close();
  return ans === 'y' || ans === 'yes';
}

// ---------- 展示 ----------

function printList(setting, profiles) {
  const cur = setting.auth;
  if (hasLogin(cur)) {
    console.log(`当前登录：${cur.name || '(未命名)'}  <${cur.userId}>  ${expInfo(cur)}`);
  } else {
    console.log('当前登录：（未登录）');
  }
  if (!profiles.length) {
    console.log('还没有保存任何账号快照。先运行： mirasim-accounts save <名字>');
    return;
  }
  console.log('');
  profiles.forEach((p, i) => {
    const mark = hasLogin(cur) && p.userId === cur.userId ? '●' : ' ';
    console.log(
      `  ${mark} [${i + 1}] ${p.name}` +
      `  （${p.accountName || '?'} / ${shortId(p.userId)}…，保存于 ${fmtTime(p.savedAt)}）`
    );
  });
  console.log('\n  ● = 当前账号。切换： mirasim-accounts use <名字或序号>');
}

// ---------- 子命令 ----------

function cmdWhoami() {
  const setting = loadSetting();
  const a = setting.auth;
  if (!hasLogin(a)) {
    console.log('（未登录）');
    return;
  }
  console.log(`账号：${a.name || '(未命名)'}`);
  console.log(`userId：${a.userId}`);
  if (a.exp) console.log(expInfo(a));
  const hit = listProfiles().find((p) => p.userId === a.userId);
  console.log(hit ? `对应快照：「${hit.name}」（保存于 ${fmtTime(hit.savedAt)}）` : '对应快照：无（建议先 save 一次）');
}

function cmdList() {
  printList(loadSetting(), listProfiles());
}

function cmdSave(nameArg) {
  const setting = loadSetting();
  if (!hasLogin(setting.auth)) fail('当前没有登录，没什么可保存的。请先在 Mirasim 里登录。');
  const auth = setting.auth;
  const profiles = listProfiles();
  const sameUser = profiles.find((p) => p.userId === auth.userId);

  let name = sanitizeName(nameArg);
  if (!name) name = sameUser?.name || sanitizeName(auth.name) || 'usr-' + shortId(auth.userId);

  const clash = profiles.find((p) => p.name === name && p.userId !== auth.userId);
  if (clash) fail(`快照名「${name}」已被另一个账号（${clash.accountName} / ${shortId(clash.userId)}…）占用，换个名字。`);

  if (sameUser && sameUser.name !== name) {
    fs.rmSync(sameUser.file); // 同一账号改名保存：移除旧名，保持一账号一快照
    console.log(`（快照改名：「${sameUser.name}」→「${name}」）`);
  }
  saveProfile(name, auth);
  console.log(`✔ 已保存当前账号 ${auth.name || ''} <${shortId(auth.userId)}…> → 快照「${name}」`);
}

async function cmdUse(key) {
  if (!key) fail('用法：mirasim-accounts use <快照名或序号>');
  const setting = loadSetting();
  const profiles = listProfiles();
  if (!profiles.length) fail('还没有任何快照。先在每个账号登录状态下运行 save。');

  let target = profiles.find((p) => p.name === key);
  if (!target && /^\d+$/.test(key)) target = profiles[Number(key) - 1];
  if (!target) fail(`找不到快照「${key}」。运行 list 查看现有快照。`);

  if (hasLogin(setting.auth) && setting.auth.userId === target.userId) {
    saveProfile(target.name, setting.auth); // 顺手刷新快照
    console.log(`已经在使用「${target.name}」（${target.accountName}），无需切换；快照已刷新。`);
    return;
  }

  if (mirasimRunning()) {
    console.log('⚠ Mirasim 正在运行。切换会被服务端热重载，但正在跑的会话可能出现短暂异常，');
    console.log('  且若此刻应用恰好在刷新令牌，可能覆盖本次切换。更稳妥的做法是先退出 Mirasim。');
    if (!(await confirm('仍要在运行状态下切换吗？'))) {
      console.log('已取消。退出 Mirasim 后重试，或加 --force 强制执行。');
      return;
    }
  }

  // 1) 回存当前登录态，2) 备份，3) 写入目标 auth
  const note = snapshotCurrent(setting, profiles);
  if (note) console.log('• ' + note);
  const bak = backupSetting();
  console.log('• 已备份 setting.json → ' + path.relative(HOME, bak));

  const fresh = loadSetting(); // 写前重读，尽量缩小与应用写入的竞争窗口
  fresh.auth = target.auth;
  writeJsonAtomic(SETTING, fresh);

  console.log(`✔ 已切换到「${target.name}」：${target.accountName || '?'} <${target.userId}>`);
  console.log(mirasimRunning()
    ? '  Mirasim 会在几秒内热重载配置；建议重启 Mirasim 以确保所有连接使用新账号。'
    : '  下次启动 Mirasim 即以该账号登录，无需重新输码。');
  const info = expInfo(target.auth);
  if (info) console.log('  ' + info);
}

async function cmdRemove(name) {
  if (!name) fail('用法：mirasim-accounts rm <快照名>');
  const profiles = listProfiles();
  const target = profiles.find((p) => p.name === name) || (/^\d+$/.test(name) ? profiles[Number(name) - 1] : null);
  if (!target) fail(`找不到快照「${name}」。`);
  const setting = loadSetting();
  if (hasLogin(setting.auth) && setting.auth.userId === target.userId) {
    console.log('（注意：这是当前正在使用的账号，删除快照不影响当前登录，只是无法再切回。）');
  }
  if (!(await confirm(`删除快照「${target.name}」（${target.accountName} / ${shortId(target.userId)}…）？`))) {
    console.log('已取消。');
    return;
  }
  fs.rmSync(target.file);
  console.log(`✔ 已删除快照「${target.name}」`);
}

async function cmdRestore(which) {
  if (!fs.existsSync(BACKUP_DIR)) fail('没有任何备份。');
  const all = fs.readdirSync(BACKUP_DIR).filter((f) => f.startsWith('setting-')).sort().reverse();
  if (!all.length) fail('没有任何备份。');
  if (!which) {
    console.log('可用备份（新→旧）：');
    all.forEach((f, i) => console.log(`  [${i + 1}] ${f}`));
    console.log('恢复： mirasim-accounts restore <序号或文件名>（1 = 最新）');
    return;
  }
  const file = /^\d+$/.test(which) ? all[Number(which) - 1] : all.find((f) => f === which || f === which + '.json');
  if (!file) fail(`找不到备份「${which}」。`);
  if (!(await confirm(`用 ${file} 覆盖当前 setting.json ？`))) { console.log('已取消。'); return; }
  const obj = readJson(path.join(BACKUP_DIR, file)); // 顺带校验 JSON 完整性
  backupSetting();
  writeJsonAtomic(SETTING, obj);
  console.log(`✔ 已从 ${file} 恢复 setting.json（恢复前的版本也已备份）。建议重启 Mirasim。`);
}

async function cmdInteractive() {
  const setting = loadSetting();
  const profiles = listProfiles();
  printList(setting, profiles);
  if (!process.stdin.isTTY) return;
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const ans = (await rl.question('\n输入序号切换账号，s = 保存当前登录为快照，回车退出：')).trim();
  rl.close();
  if (!ans) return;
  if (ans.toLowerCase() === 's') {
    cmdSave();
  } else if (/^\d+$/.test(ans)) {
    await cmdUse(ans);
  } else {
    console.log('未识别的输入，已退出。');
  }
}

function cmdHelp() {
  console.log(`mirasim 订阅账号切换器 —— 保存/切换 ~/.mirasim 的登录态，免去重复登录

用法：
  mirasim-accounts                    交互模式（列出快照，输序号切换）
  mirasim-accounts list               列出快照与当前账号
  mirasim-accounts whoami             显示当前登录的账号
  mirasim-accounts save [名字]        把当前登录保存为快照（同账号重复 save = 刷新）
  mirasim-accounts use <名字|序号>    切换到某个快照（自动回存当前账号 + 备份 setting.json）
  mirasim-accounts rm <名字|序号>     删除快照
  mirasim-accounts restore [序号]     查看/恢复 setting.json 备份

选项：
  --home <目录>   指定 mirasim 数据目录（默认 %MIRASIM_HOME% 或 ~/.mirasim）
  --force / -f    Mirasim 运行中也直接切换，不询问
  --yes / -y      跳过确认

首次使用：在账号 A 登录状态下 save A，退出登录换账号 B 登录后 save B，
之后随时 use A / use B 即可来回切换，无需再收验证码。
注意：快照里的令牌由本机 secret.key 加密，仅本机有效，换电脑需重新登录。`);
}

// ---------- 入口 ----------

const cmd = positional[0] || '';
const arg = positional[1];

try {
  switch (cmd) {
    case '': await cmdInteractive(); break;
    case 'list': case 'ls': cmdList(); break;
    case 'whoami': case 'current': cmdWhoami(); break;
    case 'save': cmdSave(arg); break;
    case 'use': case 'switch': await cmdUse(arg); break;
    case 'rm': case 'remove': case 'delete': await cmdRemove(arg); break;
    case 'restore': await cmdRestore(arg); break;
    case 'help': cmdHelp(); break;
    default: fail(`未知命令「${cmd}」。运行 help 查看用法。`);
  }
} catch (e) {
  fail(e?.message || String(e));
}
