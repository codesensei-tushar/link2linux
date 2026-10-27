(() => {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  if (/Android/i.test(navigator.userAgent)) {
    document.documentElement.dataset.platform = "android";
    const primary = document.getElementById("hero-download-primary");
    const secondary = document.getElementById("hero-download-secondary");
    if (primary) {
      primary.textContent = "Download for Android";
      primary.dataset.asset = "link2linux.apk";
      primary.href =
        "https://github.com/codesensei-tushar/link2linux/releases/download/v0.4.0/link2linux.apk";
    }
    if (secondary) {
      secondary.textContent = "Linux downloads";
      secondary.href = "#linux";
    }
  }

  const live = document.getElementById("copy-live");
  document.querySelectorAll(".code").forEach((block) => {
    const code = block.querySelector("code");
    if (!code) return;
    if (block.querySelector(".code-copy")) return;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "code-copy";
    btn.setAttribute("aria-label", "Copy command");
    btn.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" class="icon-16"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';
    btn.addEventListener("click", async () => {
      const text = code.textContent;
      let ok = false;
      try {
        await navigator.clipboard.writeText(text);
        ok = true;
      } catch {
        const range = document.createRange();
        range.selectNodeContents(code);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
      if (ok) {
        btn.innerHTML =
          '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" class="icon-16"><path d="M20 6 9 17l-5-5"/></svg>';
        btn.classList.add("is-copied");
        if (live) live.textContent = "Copied";
        window.setTimeout(() => {
          btn.classList.remove("is-copied");
          btn.innerHTML =
            '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" class="icon-16"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';
          if (live) live.textContent = "";
        }, 1500);
      }
    });
    block.appendChild(btn);
  });

  if (!reduced) {
    const groups = [
      ".bento .card",
      ".steps .step",
      ".privacy-facts .fact",
      ".download-card",
    ];
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.remove("reveal-pending");
            observer.unobserve(entry.target);
          }
        }
      },
      { rootMargin: "0px 0px -10% 0px", threshold: 0.15 },
    );
    for (const sel of groups) {
      const nodes = document.querySelectorAll(sel);
      nodes.forEach((el, i) => {
        const rect = el.getBoundingClientRect();
        if (rect.top < window.innerHeight && rect.bottom > 0) return;
        el.classList.add("reveal", "reveal-pending");
        el.style.setProperty("--reveal-delay", `${i * 60}ms`);
        observer.observe(el);
      });
    }
  }

  const header = document.querySelector(".site-header");
  const sentinel = document.querySelector(".scroll-sentinel");
  if (header && sentinel) {
    new IntersectionObserver(
      ([entry]) => {
        header.classList.toggle("is-scrolled", !entry.isIntersecting);
      },
      { threshold: 0 },
    ).observe(sentinel);
  }
})();
