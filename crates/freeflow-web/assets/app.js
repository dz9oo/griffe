// FreeFlow — script maison, volontairement minimal (pas de framework, pas de lib de charts :
// les graphes sont du SVG rendu côté serveur). Trois responsabilités : souligner la pièce
// active après une navigation htmx, piloter la palette ⌘K, et signaler une activité humaine
// réelle au serveur pour l'auto-verrouillage (voir state.rs). Cette page n'est chargée que par
// la coque applicative (jamais par l'écran de déverrouillage).

const PIECE_OF = {
  jour: "jour",
  dashboard: "jour",
  relances: "jour",
  affaires: "affaires",
  gens: "affaires",
  prospection: "affaires",
  devis: "affaires",
  missions: "affaires",
  facturation: "affaires",
  clients: "affaires",
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

document.body.addEventListener("change", (event) => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || input.type !== "file") return;
  const name = input.closest(".pick-file")?.querySelector(".pick-name");
  if (!name) return;
  name.textContent = input.files && input.files[0] ? input.files[0].name : "le fichier";
});

document.body.addEventListener("click", (event) => {
  const pieceLink = event.target.closest(".chrome nav a[data-piece]");
  if (pieceLink) {
    markNav(pieceLink.dataset.piece);
    return;
  }
  if (event.target.closest(".mark")) markNav("jour");

  const geste = event.target.closest("ol.gestes .geste, ol.papers .paper-fiche");
  if (geste && !event.target.closest(".unfold")) {
    const item = geste.closest("li");
    if (item) {
      item.classList.toggle("open");
      const expanded = item.classList.contains("open");
      geste.setAttribute("aria-expanded", expanded ? "true" : "false");
      item.parentElement?.querySelectorAll(":scope > li").forEach((other) => {
        if (other !== item) {
          other.classList.remove("open");
          other.querySelector(".geste, .paper-fiche")?.setAttribute("aria-expanded", "false");
        }
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

function dismissNativeDatePicker(input) {
  if (!(input instanceof HTMLInputElement) || input.type !== "date") return;
  input.blur();
}

document.body.addEventListener("change", (event) => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || input.type !== "date") return;
  requestAnimationFrame(() => dismissNativeDatePicker(input));
});

const MONTHS_FR = [
  "janvier", "février", "mars", "avril", "mai", "juin",
  "juillet", "août", "septembre", "octobre", "novembre", "décembre",
];

function pad2(n) {
  return String(n).padStart(2, "0");
}

function isoDate(year, month, day) {
  return `${year}-${pad2(month)}-${pad2(day)}`;
}

function parseIsoDate(iso) {
  const [year, month, day] = iso.split("-").map(Number);
  if (!year || !month || !day) return null;
  return { year, month, day };
}

function formatDateFr(iso) {
  const parts = parseIsoDate(iso);
  if (!parts) return iso;
  return `${parts.day} ${MONTHS_FR[parts.month - 1]} ${parts.year}`;
}

function closeDatePick(root) {
  const pop = root.querySelector(".date-pick-pop");
  const toggle = root.querySelector(".date-pick-toggle");
  if (pop) pop.hidden = true;
  if (toggle) toggle.setAttribute("aria-expanded", "false");
}

function renderDatePop(root) {
  const pop = root.querySelector(".date-pick-pop");
  const input = root.querySelector("input");
  if (!pop || !input) return;
  const selected = parseIsoDate(input.value) || parseIsoDate(root.dataset.max || "") || {
    year: 2026, month: 1, day: 1,
  };
  const view = parseIsoDate(`${pop.dataset.view}-01`) || selected;
  const max = root.dataset.max || "";
  const first = new Date(view.year, view.month - 1, 1);
  const empty = (first.getDay() + 6) % 7;
  const lastDay = new Date(view.year, view.month, 0).getDate();
  const title = `${MONTHS_FR[view.month - 1]} ${view.year}`;
  let cells = "";
  for (let i = 0; i < empty; i += 1) cells += '<div class="d empty"></div>';
  for (let day = 1; day <= lastDay; day += 1) {
    const iso = isoDate(view.year, view.month, day);
    const off = max && iso > max;
    const sel = iso === input.value;
    const cls = `d${off ? " off" : ""}${sel ? " sel" : ""}`;
    const isoAttr = off ? "" : ` data-iso="${iso}"`;
    cells += `<button type="button" class="${cls}"${isoAttr}>${day}</button>`;
  }
  pop.innerHTML = `<div class="date-pick-nav"><button type="button" data-nav="-1" aria-label="Mois précédent">‹</button><span>${title}</span><button type="button" data-nav="1" aria-label="Mois suivant">›</button></div><div class="cal"><div class="dow">Lu</div><div class="dow">Ma</div><div class="dow">Me</div><div class="dow">Je</div><div class="dow">Ve</div><div class="dow">Sa</div><div class="dow">Di</div>${cells}</div>`;
  pop.dataset.view = `${view.year}-${pad2(view.month)}`;
}

function openDatePick(root) {
  document.querySelectorAll(".date-pick").forEach((other) => {
    if (other !== root) closeDatePick(other);
  });
  const pop = root.querySelector(".date-pick-pop");
  const toggle = root.querySelector(".date-pick-toggle");
  const input = root.querySelector("input");
  if (!pop || !toggle) return;
  const seed = parseIsoDate(input?.value || "") || parseIsoDate(root.dataset.max || "");
  if (seed) pop.dataset.view = `${seed.year}-${pad2(seed.month)}`;
  renderDatePop(root);
  pop.hidden = false;
  toggle.setAttribute("aria-expanded", "true");
}

document.body.addEventListener("click", (event) => {
  const target = event.target;
  if (!(target instanceof Element)) return;
  const toggle = target.closest(".date-pick-toggle");
  if (toggle) {
    const root = toggle.closest(".date-pick");
    if (!root) return;
    const pop = root.querySelector(".date-pick-pop");
    if (pop && !pop.hidden) closeDatePick(root);
    else openDatePick(root);
    return;
  }
  const nav = target.closest(".date-pick-pop [data-nav]");
  if (nav) {
    const root = nav.closest(".date-pick");
    const pop = root?.querySelector(".date-pick-pop");
    if (!root || !pop) return;
    const view = parseIsoDate(`${pop.dataset.view}-01`);
    if (!view) return;
    const next = new Date(view.year, view.month - 1 + Number(nav.dataset.nav), 1);
    pop.dataset.view = `${next.getFullYear()}-${pad2(next.getMonth() + 1)}`;
    renderDatePop(root);
    return;
  }
  const day = target.closest(".date-pick-pop .d[data-iso]");
  if (day) {
    const root = day.closest(".date-pick");
    const input = root?.querySelector("input");
    const button = root?.querySelector(".date-pick-toggle");
    const iso = day.getAttribute("data-iso");
    if (!root || !input || !button || !iso) return;
    input.value = iso;
    button.textContent = formatDateFr(iso);
    closeDatePick(root);
  }
});

document.body.addEventListener("pointerdown", (event) => {
  const onPick = event.target instanceof Element
    ? event.target.closest(".date-pick")
    : null;
  document.querySelectorAll(".date-pick").forEach((root) => {
    if (root !== onPick) closeDatePick(root);
  });
  const onNative = event.target instanceof Element
    ? event.target.closest("input[type=date]")
    : null;
  document.querySelectorAll("input[type=date]").forEach((input) => {
    if (input !== onNative) dismissNativeDatePicker(input);
  });
});

document.body.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  document.querySelectorAll(".date-pick").forEach(closeDatePick);
  dismissNativeDatePicker(document.activeElement);
});
