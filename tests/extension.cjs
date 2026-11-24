const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");

const code = fs.readFileSync(path.join(__dirname, "../browser/extension/background.js"), "utf8");
const manifest = JSON.parse(fs.readFileSync(
  path.join(__dirname, "../browser/extension/manifest.json"), "utf8"));
const url = "https://example.test/file.zip?sig=private";

function harness({ off = false, native = true } = {}) {
  const calls = [];
  const hooks = new Map();
  const register = (key) => ({ addListener: (listener) => hooks.set(key, listener) });
  const store = { off };
  const port = {
    postMessage: (message) => calls.push({ message }),
    disconnect: () => calls.push("disconnect"),
    onMessage: register("reply"),
    onDisconnect: register("disconnect"),
  };
  const record = (name) => async (value) => { calls.push({ [name]: value }); };
  vm.runInNewContext(code, {
    URL, setTimeout: () => 1, clearTimeout: () => {},
    chrome: {
      action: {
        onClicked: register("toolbar"),
        setBadgeText: ({ text }) => { hooks.set("badge", text); },
        setBadgeBackgroundColor: () => {}, setTitle: () => {},
      },
      storage: { local: {
        get: async () => ({ ...store }),
        set: async (values) => { Object.assign(store, values); },
      } },
      cookies: { getAll: async (filter) => {
        assert.equal(filter.url, url);
        return [{ name: "session", value: "abc" }, { name: "theme", value: "dark" }];
      } },
      contextMenus: {
        create: (item, callback) => { hooks.set("menu", item); callback(); },
        onClicked: register("menu-click"),
      },
      downloads: {
        onDeterminingFilename: register("filename"),
        pause: record("paused"), resume: record("resumed"),
        cancel: record("cancelled"), erase: record("erased"),
      },
      runtime: { onInstalled: register("installed"),
        connectNative: (host) => {
          assert.equal(host, "com.parally.download");
          if (!native) throw new Error("Native host is not registered.");
          return port;
        } },
    },
  });
  return { calls, hooks, store };
}

const settle = () => new Promise(setImmediate);
const plain = (value) => JSON.parse(JSON.stringify(value));

async function download(current, changes = {}) {
  current.suggested = false;
  current.hooks.get("filename")({
    id: 7, url, finalUrl: url, filename: "file.zip", state: "in_progress",
    referrer: "https://example.test/page", ...changes,
  }, () => { current.suggested = true; });
  for (let step = 0; step < 5; step += 1) await settle();
}

test("the fixed key matches the extension ID the native host trusts", () => {
  const der = Buffer.from(manifest.key, "base64");
  const digest = require("node:crypto").createHash("sha256").update(der).digest();
  const id = [...digest.subarray(0, 16)]
    .map((byte) => String.fromCharCode(97 + (byte >> 4), 97 + (byte & 15))).join("");
  const platform = fs.readFileSync(path.join(__dirname, "../src/platform.rs"), "utf8");
  assert.ok(platform.includes(`EXTENSION_ID: &str = "${id}"`), id);
});

test("pauses the browser copy, sends cookies for this file, then cancels it", async () => {
  const current = harness();
  await download(current);
  assert.deepEqual(plain(current.calls), [{ paused: 7 }, { message: {
    url, filename: "file.zip", referrer: "https://example.test/page",
    cookie: "session=abc; theme=dark",
  } }]);
  assert.ok(!current.suggested, "The browser's Save As dialog waits for Parally.");
  current.hooks.get("reply")({ accepted: true });
  for (let step = 0; step < 5; step += 1) await settle();
  assert.deepEqual(plain(current.calls.slice(2)),
    ["disconnect", { cancelled: 7 }, { erased: { id: 7 } }]);
  assert.ok(current.suggested);
});

test("resumes the browser copy when Parally declines or is not installed", async () => {
  for (const outcome of ["declined", "closed", "missing"]) {
    const current = harness({ native: outcome !== "missing" });
    await download(current);
    if (outcome === "declined") current.hooks.get("reply")({ accepted: false });
    if (outcome === "closed") current.hooks.get("disconnect")();
    await settle(); await settle();
    for (let step = 0; step < 5; step += 1) await settle();
    assert.deepEqual(plain(current.calls.at(-1)), { resumed: 7 }, outcome);
    assert.ok(current.suggested, outcome);
    assert.ok(!current.calls.some((entry) => entry.cancelled), outcome);
  }
});

test("leaves blob links, finished items, repeats and the off state to the browser", async () => {
  for (const changes of [{ finalUrl: "blob:https://example.test/a" }, { state: "complete" }]) {
    const current = harness();
    await download(current, changes);
    assert.deepEqual(current.calls, []);
    assert.ok(current.suggested);
  }
  const off = harness({ off: true });
  await download(off);
  assert.deepEqual(off.calls, []);
  assert.ok(off.suggested);
  const twice = harness();
  await download(twice);
  await download(twice);
  assert.equal(twice.calls.filter((entry) => entry.paused).length, 1);
});

test("the toolbar button turns handoff off and on", async () => {
  const current = harness();
  await settle();
  assert.equal(current.hooks.get("badge"), "");
  await current.hooks.get("toolbar")();
  assert.equal(current.store.off, true);
  assert.equal(current.hooks.get("badge"), "off");
  await current.hooks.get("toolbar")();
  assert.equal(current.store.off, false);
  assert.equal(current.hooks.get("badge"), "");
});

test("the link menu sends a direct link and fixes Hugging Face file views", async () => {
  const current = harness();
  current.hooks.get("installed")({ reason: "update" });
  const item = current.hooks.get("menu");
  assert.equal(item.title, "Download with Parally");
  assert.equal(item.contexts.join(), "link");
  const click = current.hooks.get("menu-click");
  click({ menuItemId: item.id, linkUrl: "blob:https://example.test/a" });
  click({ menuItemId: item.id, linkUrl: "https://user:secret@example.test/a" });
  await settle();
  assert.deepEqual(current.calls, []);
  click({ menuItemId: item.id, linkUrl: url, pageUrl: "https://example.test/page" });
  await settle(); await settle();
  assert.deepEqual(plain(current.calls), [{ message: {
    url, filename: "", referrer: "https://example.test/page",
    cookie: "session=abc; theme=dark", manual: true,
  } }]);
  const hub = harness();
  const view = "https://huggingface.co/example/model/blob/main/mmproj-F16.gguf";
  hub.hooks.get("menu-click")({ menuItemId: "parally-download-link", linkUrl: view });
  await settle(); await settle();
  assert.equal(hub.calls[0].message.url, view.replace("/blob/", "/resolve/"));
});
