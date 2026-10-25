// Points the download buttons at the files of the newest release, including
// pre-releases. Without JavaScript, before the first release, or when GitHub
// can't be reached, the buttons lead to the releases page instead.
(async () => {
  const status = document.getElementById("release-status");
  try {
    const res = await fetch("https://api.github.com/repos/codesensei-tushar/link2linux/releases?per_page=1");
    const [release] = res.ok ? await res.json() : [];
    if (!release) return;
    const assets = new Map(release.assets.map((asset) => [asset.name, asset.browser_download_url]));
    for (const link of document.querySelectorAll("a[data-asset]")) {
      const url = assets.get(link.dataset.asset);
      if (url) {
        link.href = url;
        link.classList.remove("pending");
      }
    }
    const date = new Date(release.published_at).toLocaleDateString(undefined, { dateStyle: "long" });
    const kind = release.prerelease ? "Pre-release" : "Latest release";
    status.textContent = `${kind} ${release.tag_name}, published ${date}.`;
  } catch {
    // Keep the links to the releases page.
  }
})();
