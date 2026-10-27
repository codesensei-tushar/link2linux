// The download buttons link straight to the files of the release named in
// index.html, so they work without JavaScript. This points them at the newest
// release instead, including pre-releases, in case a newer one is out.
(async () => {
  const status = document.getElementById("release-status");
  try {
    const res = await fetch("https://api.github.com/repos/codesensei-tushar/link2linux/releases?per_page=1");
    const [release] = res.ok ? await res.json() : [];
    if (!release) return;
    const assets = new Map(release.assets.map((asset) => [asset.name, asset.browser_download_url]));
    for (const link of document.querySelectorAll("a[data-asset]")) {
      const url = assets.get(link.dataset.asset);
      if (url) link.href = url;
    }
    const date = new Date(release.published_at).toLocaleDateString(undefined, { dateStyle: "long" });
    const kind = release.prerelease ? "Pre-release" : "Latest release";
    const releaseText = status?.querySelector("[data-release-text]");
    if (releaseText) {
      releaseText.textContent = `${kind} ${release.tag_name}, published ${date}.`;
    }
    for (const el of document.querySelectorAll("[data-release-tag]")) {
      el.textContent = release.tag_name;
    }
  } catch {
    // Keep the links to the release named in index.html.
  }
})();
