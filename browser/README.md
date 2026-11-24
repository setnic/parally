# Parally browser extension

One Manifest V3 extension for Chrome, Edge, Brave, Vivaldi, Opera and Chromium. It hands browser downloads to Parally, which asks where to save each file.

## Add it to a browser

1. In Parally, open **Settings → Add Parally to a browser…** and click **Open extension folder**. Parally copies the folder path.
2. Open the browser's extensions page (`chrome://extensions`, `edge://extensions`, `brave://extensions`) and turn on **Developer mode**.
3. Click **Load unpacked** and choose that folder.

That's all. Parally connects itself to the browsers each time it starts, so nothing else needs to run. The extension has a fixed ID, so it keeps working after Parally moves or updates. After an update, click the reload button on the extension card.

## How it works

- **Downloads go to Parally.** When the browser starts a download, the extension pauses it and Parally asks for a folder. **Start download** cancels the browser copy. **Keep in browser** resumes it. If Parally is closed, it opens. If Parally is missing or the server refuses it, the browser download resumes.
- **The Parally toolbar button** turns handoff off ("off" badge) and back on.
- **Right-click a link → Download with Parally** sends any HTTP(S) link. Hugging Face file pages are turned into direct file links.

## Privacy

Parally receives the file URL, the suggested file name, the page address (Referer) and the browser's cookies for that URL only. That lets downloads behind a site login and signed links work. Parally uses the cookies only for that download: they live in memory while Parally runs and are never saved in its history or checkpoints. They pass through a short-lived file in Parally's data folder, deleted as soon as Parally answers. Parally does not send the cookies to other sites after a redirect.

A download resumed after Parally restarts has no cookies, so a site that needs a login may refuse it.

## Limits

- `blob:` and `data:` downloads stay in the browser.
- Parally repeats the request as a plain GET. Downloads that came from a submitted form (POST) may fail in Parally. Choose **Keep in browser** for those.
- On Windows, Opera uses the Chrome registration. Other Chromium forks need standard Native Messaging support.

Uninstalling Parally on Windows removes the browser registrations. Remove the extension card in each browser yourself.
