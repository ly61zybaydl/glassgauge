import { test } from "node:test";
import assert from "node:assert/strict";
import { accountItems, currentLabel, savedAgo, esc } from "../accounts-view.js";

const NOW = 1_700_000_000_000;

test("currentLabel：邮箱优先 / 显示名 / userId 兜底 / 未登录", () => {
  assert.equal(currentLabel(null), "未登录");
  assert.equal(currentLabel({ current: null, profiles: [] }), "未登录");
  assert.equal(currentLabel({ current: { email: "alice@example.com", name: "Ada Lovelace", userId: "usr_1" } }), "alice@example.com");
  assert.equal(currentLabel({ current: { email: null, name: "Ada Lovelace", userId: "usr_1" } }), "Ada Lovelace");
  assert.equal(currentLabel({ current: { name: "", userId: "usr_1" } }), "usr_1");
});

test("accountItems：当前置顶，其余按名排序，sub 邮箱优先、回退账号名", () => {
  const view = {
    profiles: [
      { name: "乙", email: "b@x.com", accountName: "B", savedAt: NOW - 5 * 60_000, current: false },
      { name: "甲", email: "a@x.com", accountName: "A", savedAt: NOW - 2 * 3600_000, current: true },
      { name: "丙", email: null, accountName: "C", savedAt: 0, current: false },
    ],
  };
  const items = accountItems(view, NOW);
  assert.deepEqual(items.map((i) => i.name), ["甲", "丙", "乙"]);
  assert.ok(items[0].current);
  assert.equal(items[2].sub, "b@x.com · 5 分钟前");
  assert.equal(items[1].sub, "C · 时间未知"); // 无邮箱回退账号名
});

test("savedAgo 分档", () => {
  assert.equal(savedAgo(NOW - 10_000, NOW), "刚保存");
  assert.equal(savedAgo(NOW - 90_000, NOW), "1 分钟前");
  assert.equal(savedAgo(NOW - 3 * 3600_000, NOW), "3 小时前");
  assert.equal(savedAgo(NOW - 5 * 86400_000, NOW), "5 天前");
  assert.match(savedAgo(NOW - 400 * 86400_000, NOW), /^\d{4}-\d{2}-\d{2}$/);
});

test("esc 转义全部危险字符", () => {
  assert.equal(esc(`<b a="1" b='2'>&`), "&lt;b a=&quot;1&quot; b=&#39;2&#39;&gt;&amp;");
  assert.equal(esc(null), "");
});
