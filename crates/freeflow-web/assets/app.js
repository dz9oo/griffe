// FreeFlow — script maison, volontairement minimal (pas de framework, pas de lib de charts :
// les graphes sont du SVG rendu côté serveur). Trois responsabilités : souligner la pièce
// active après une navigation htmx, piloter la palette ⌘K, et signaler une activité humaine
// réelle au serveur pour l'auto-verrouillage (voir state.rs). Cette page n'est chargée que par
// la coque applicative (jamais par l'écran de déverrouillage).

const PIECE_OF = {
  jour: "jour",
  dashboard: "jour",
  relances: "jour",
  gens: "gens",
  prospection: "gens",
  devis: "gens",
  missions: "gens",
  facturation: "gens",
  clients: "gens",
  societe: "societe",
  depenses: "societe",
  cloture: "societe",
};

const WIDE = new Set(["societe", "depenses", "cloture", "console"]);

function viewSlugFrom(root) {
  return root?.querySelector?.("[data-view]")?.dataset.view
    || root?.dataset?.view
    || "";
}

function markNav(slug) {
  const piece = PIECE_OF[slug] || "";
  document.querySelectorAll(".chrome nav a[data-piece]").forEach((link) => {
    if (link.dataset.piece === piece) {
      link.setAttribute("aria-current", "page");
    } else {
      link.removeAttribute("aria-current");
    }
  });
  const content = document.getElementById("content");
  if (content) content.classList.toggle("wide", WIDE.has(slug));
}

document.body.addEventListener("click", (event) => {
  const pieceLink = event.target.closest(".chrome nav a[data-piece]");
  if (pieceLink) {
    markNav(pieceLink.dataset.piece);
    return;
  }
  if (event.target.closest(".mark")) markNav("jour");

  const geste = event.target.closest("ol.gestes .geste");
  if (geste && !event.target.closest(".unfold")) {
    const item = geste.closest("li");
    if (item) {
      item.classList.toggle("open");
      item.parentElement?.querySelectorAll(":scope > li").forEach((other) => {
        if (other !== item) other.classList.remove("open");
      });
    }
    return;
  }

  const dayBtn = event.target.closest(".cal .d[data-day]");
  if (dayBtn) {
    const day = dayBtn.dataset.day;
    document.querySelectorAll(".cal .d.on").forEach((el) => el.classList.remove("on"));
    document.querySelectorAll(".agenda button.on, .agenda a.on").forEach((el) => {
      el.classList.remove("on");
    });
    dayBtn.classList.add("on");
    document.querySelectorAll(`.agenda [data-day="${day}"]`).forEach((el) => {
      el.classList.add("on");
    });
  }
});

document.body.addEventListener("htmx:afterSwap", (event) => {
  if (event.detail?.target?.id !== "content") return;
  const slug = viewSlugFrom(event.detail.target);
  if (slug) markNav(slug);
});

const paletteOverlay = document.getElementById("palette-overlay");
const paletteInput = document.getElementById("palette-input");
const paletteHint = document.querySelector(".palette-hint");

function closePalette() {
  if (!paletteOverlay || !paletteInput) return;
  paletteOverlay.classList.remove("open");
  paletteInput.value = "";
  filterPalette();
}

function openPalette() {
  if (!paletteOverlay || !paletteInput) return;
  paletteOverlay.classList.add("open");
  paletteInput.focus();
}

function filterPalette() {
  if (!paletteInput) return;
  const query = paletteInput.value.trim().toLowerCase();
  const items = Array.from(document.querySelectorAll(".palette-item"));
  let firstVisible = null;
  items.forEach((item) => {
    const match = item.dataset.label.includes(query);
    item.style.display = match ? "" : "none";
    item.classList.remove("sel");
    if (match && !firstVisible) firstVisible = item;
  });
  if (firstVisible) firstVisible.classList.add("sel");
}

if (paletteHint) {
  paletteHint.addEventListener("click", openPalette);
}

if (paletteOverlay && paletteInput) {
  document.addEventListener("keydown", (event) => {
    const isPaletteShortcut = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k";
    if (isPaletteShortcut) {
      event.preventDefault();
      paletteOverlay.classList.contains("open") ? closePalette() : openPalette();
      return;
    }
    if (!paletteOverlay.classList.contains("open")) return;
    if (event.key === "Escape") {
      closePalette();
    } else if (event.key === "Enter") {
      const selected = document.querySelector(".palette-item.sel");
      if (selected) {
        closePalette();
        selected.click();
      }
    }
  });

  paletteInput.addEventListener("input", filterPalette);
  paletteOverlay.addEventListener("click", (event) => {
    if (event.target === paletteOverlay) closePalette();
  });
}

const consoleLog = document.getElementById("console-log");
if (consoleLog) {
  document.body.addEventListener("htmx:afterSwap", (event) => {
    if (event.target.id === "console-log") {
      consoleLog.scrollTop = consoleLog.scrollHeight;
    }
  });
}

document.body.addEventListener("htmx:afterRequest", (event) => {
  if (event.target.id === "console-form") {
    event.target.reset();
    event.target.querySelector("input")?.focus();
  }
});

const panel = document.getElementById("panel");
function closePanel() {
  panel?.replaceChildren();
}
if (panel) {
  document.addEventListener("click", (event) => {
    if (panel.childElementCount === 0) return;
    if (event.target.closest("#panel")) {
      if (event.target.closest(".panel-close")) closePanel();
      return;
    }
    closePanel();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && panel.childElementCount > 0) closePanel();
  });
  document.body.addEventListener("freeflow:saved", closePanel);
}

const NEW_ACTION_BY_VIEW = {
  clients: "/clients/new",
  prospection: "/prospection/new",
  gens: "/prospection/new",
  missions: "/missions/new",
  devis: "/devis/new",
  depenses: "/depenses/new",
  cloture: "/cloture/new",
};
document.addEventListener("keydown", (event) => {
  const view = document.querySelector("[data-view]")?.dataset.view;
  const typing = event.target.tagName === "INPUT" || event.target.tagName === "TEXTAREA" || event.target.tagName === "SELECT";
  if (view === "relances" && !typing && !event.metaKey && !event.ctrlKey && !event.altKey) {
    const action = ({ Enter: "draft", e: "sent", s: "snooze-tomorrow", k: "skip" })[event.key];
    if (action) {
      const btn = document.querySelector(`[data-follow-action="${action}"]`);
      if (btn) {
        event.preventDefault();
        btn.click();
        return;
      }
    }
  }
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "?" || event.metaKey || event.ctrlKey || event.altKey) return;
  const target = event.target;
  if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT") return;
  if (paletteOverlay?.classList.contains("open")) return;
  const link = document.getElementById("aide-link");
  if (!link) return;
  event.preventDefault();
  link.click();
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "n" || event.metaKey || event.ctrlKey || event.altKey) return;
  const target = event.target;
  if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT") return;
  if (paletteOverlay?.classList.contains("open")) return;
  const activeView = document.querySelector("[data-view]")?.dataset.view;
  const action = activeView && NEW_ACTION_BY_VIEW[activeView];
  if (!action || !panel) return;
  event.preventDefault();
  htmx.ajax("GET", action, { target: "#panel", swap: "innerHTML" });
});

(() => {
  let lastTouch = 0;
  const TOUCH_MIN_INTERVAL_MS = 60_000;

  function touchSession() {
    const now = Date.now();
    if (now - lastTouch < TOUCH_MIN_INTERVAL_MS) return;
    lastTouch = now;
    fetch("/session/touch", { method: "POST" }).catch(() => {
      // Rien à faire : si la session a expiré entre-temps, la prochaine navigation ou le
      // prochain battement du rail d'audit redirigera vers /unlock de toute façon.
    });
  }

  document.addEventListener("keydown", touchSession);
  document.addEventListener("pointerdown", touchSession);
})();

// Thème sombre du lot 46 : hors v1 Atelier. Une préférence locale éventuelle n'est plus lue.

const bodyGrid = document.getElementById("body-grid");
const auditToggle = document.getElementById("audit-toggle");
function applyAudit() {
  if (!bodyGrid) return;
  const open = localStorage.getItem("freeflow.audit") === "open";
  bodyGrid.classList.toggle("audit-collapsed", !open);
  if (auditToggle) auditToggle.textContent = open ? "Masquer le journal" : "Journal";
}
applyAudit();
auditToggle?.addEventListener("click", () => {
  const open = localStorage.getItem("freeflow.audit") === "open";
  localStorage.setItem("freeflow.audit", open ? "closed" : "open");
  applyAudit();
});

document.body.addEventListener("change", (event) => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || input.type !== "date") return;
  requestAnimationFrame(() => input.blur());
});
