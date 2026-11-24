"use strict";

const host = "com.parally.download";
const menu = "parally-download-link";
const handled = new Set();

function webUrl(text) {
  try {
    const url = new URL(text);
    return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password
      ? url.href : "";
  } catch {
    return "";
  }
}

function downloadLink(text) {
  const safe = webUrl(text);
  if (!safe) return "";
  const url = new URL(safe);
  if (url.protocol === "https:" && url.hostname === "huggingface.co") {
    url.pathname = url.pathname.replace(
      /^\/(?:datasets\/|spaces\/)?[^/]+\/[^/]+\/blob\/(?=[^/]+\/.+)/,
      (path) => path.replace(/blob\/$/, "resolve/"),
    );
  }
  return url.href;
}

// Cookies go to Parally for this one file only; Parally keeps them in memory.
async function cookieHeader(url) {
  try {
    const cookies = await chrome.cookies.getAll({ url });
    return cookies.map((cookie) => `${cookie.name}=${cookie.value}`).join("; ");
  } catch {
    return "";
  }
}

// Resolves true once Parally has the file, false when it is declined or unavailable.
async function sendToParally(url, filename, referrer, manual) {
  const message = { url, filename, referrer: webUrl(referrer), cookie: await cookieHeader(url) };
  if (manual) message.manual = true;
  return new Promise((resolve) => {
    let port;
    try { port = chrome.runtime.connectNative(host); } catch { resolve(false); return; }
    let timer;
    const finish = (accepted) => {
      clearTimeout(timer);
      try { port.disconnect(); } catch { /* Already closed. */ }
      resolve(accepted);
    };
    timer = setTimeout(() => finish(false), 305_000);
    port.onDisconnect.addListener(() => {
      void chrome.runtime.lastError;
      finish(false);
    });
    port.onMessage.addListener((reply) => finish(reply?.accepted === true));
    try { port.postMessage(message); } catch { finish(false); }
  });
}

async function isOff() {
  return (await chrome.storage.local.get("off")).off === true;
}

function showState(off) {
  chrome.action.setBadgeText({ text: off ? "off" : "" });
  chrome.action.setBadgeBackgroundColor({ color: "#6b7280" });
  chrome.action.setTitle({ title: off
    ? "Parally is off: the browser keeps downloads. Click to turn on."
    : "Parally is on: downloads open in Parally. Click to turn off." });
}

chrome.action.onClicked.addListener(async () => {
  const off = !(await isOff());
  await chrome.storage.local.set({ off });
  showState(off);
});

function installMenu() {
  chrome.contextMenus.create({
    id: menu, title: "Download with Parally", contexts: ["link"],
    targetUrlPatterns: ["http://*/*", "https://*/*"],
  }, () => { void chrome.runtime.lastError; });
}
chrome.runtime.onInstalled.addListener(installMenu);
installMenu();
void isOff().then(showState).catch(() => {});

chrome.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId !== menu) return;
  const url = downloadLink(info.linkUrl);
  if (url) void sendToParally(url, "", info.pageUrl, true);
});

// The browser pauses its copy while Parally asks for a folder, then cancels or resumes it.
async function handOff(item, url) {
  if (await isOff()) return;
  if (handled.size >= 1024) handled.delete(handled.values().next().value);
  handled.add(item.id);
  await chrome.downloads.pause(item.id);
  const filename = (item.filename || "").split(/[\/]/).pop();
  if (await sendToParally(url, filename, item.referrer, false)) {
    await chrome.downloads.cancel(item.id);
    await chrome.downloads.erase({ id: item.id });
  } else {
    await chrome.downloads.resume(item.id);
  }
}

// Fires once the browser knows the file name, before anything is saved. Holding the name
// keeps the browser's own Save As dialog closed until Parally answers.
chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  const url = webUrl(item.finalUrl || item.url);
  if (!url || item.state !== "in_progress" || handled.has(item.id)) {
    suggest();
    return false;
  }
  void handOff(item, url).catch(() => {}).finally(() => {
    try { suggest(); } catch { /* The download was cancelled. */ }
  });
  return true;
});
