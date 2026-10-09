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
  if (!(input instanceof HTMLInputElement)) return;
  if (input.classList.contains("phrase-pick")) {
    const desk = input.closest(".phrase-desk");
    if (desk) {
      placePlayhead(desk, false);
      previewPhrase(desk);
    }
    return;
  }
  if (input.type !== "file") return;
  const name = input.closest(".pick-file")?.querySelector(".pick-name");
  if (!name) return;
  name.textContent = input.files && input.files[0] ? input.files[0].name : "le fichier";
});

document.body.addEventListener("click", (event) => {
  const token = event.target.closest("[data-insert]");
  if (token) {
    const panel = token.closest(".phrase-panel");
    const desk = token.closest(".phrase-desk");
    const active = document.activeElement;
    const field = phraseField(panel, active) || panel?.querySelector("textarea");
    if (field) {
      event.preventDefault();
      const text = token.dataset.insert || "";
      const start = field.selectionStart ?? field.value.length;
      const end = field.selectionEnd ?? start;
      field.value = field.value.slice(0, start) + text + field.value.slice(end);
      const caret = start + text.length;
      field.selectionStart = caret;
      field.selectionEnd = caret;
      field.focus();
      if (desk) previewPhrase(desk, field);
    }
    return;
  }
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

document.body.addEventListener("input", (event) => {
  const desk = event.target.closest?.(".phrase-desk");
  if (!desk) return;
  const field = event.target;
  if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement)) return;
  if (field.name.startsWith("label_")) {
    const panel = field.closest(".phrase-panel");
    const key = panel?.closest(".phrase-unit")?.querySelector(".phrase-pick")?.value;
    const tab = key && desk.querySelector(`.phrase-pick[value="${cssAttr(key)}"]`);
    const name = tab?.closest(".phrase-unit")?.querySelector(".phrase-node-name");
    if (name) name.textContent = field.value.trim() || "Ce moment";
  }
  if (field.name.startsWith("label_") || field.name.startsWith("ecart_")) refreshChronicle(desk);
});

document.body.addEventListener("htmx:afterSwap", (event) => {
  if (event.detail?.target?.id !== "content") return;
  const slug = viewSlugFrom(event.detail.target);
  if (slug) markNav(slug);
  bootPhrases(event.detail.target);
});

const JOUR_MOTS = [
  "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix",
  "onze", "douze", "treize", "quatorze", "quinze", "seize",
];

function cssAttr(value) {
  return window.CSS && CSS.escape ? CSS.escape(value) : value.replace(/["\\]/g, "\\$&");
}

function phraseField(panel, active) {
  if (!panel || !active || !panel.contains(active)) return null;
  if (active instanceof HTMLTextAreaElement) return active;
  if (active instanceof HTMLInputElement && active.name.startsWith("subject_")) return active;
  return null;
}

function bootPhrases(root) {
  const desk = root?.querySelector?.(".phrase-desk");
  if (!desk) return;
  placePlayhead(desk, true);
}

function previewPhrase(desk, field) {
  const target = field || openPanel(desk)?.querySelector("textarea");
  if (target && window.htmx) window.htmx.trigger(target, "preview");
}

function placePlayhead(desk, instant) {
  const frise = desk.querySelector(".phrase-frise");
  const head = frise?.querySelector(".phrase-playhead");
  const dot = frise?.querySelector(".phrase-pick:checked")?.closest(".phrase-unit")?.querySelector(".dot");
  if (!frise || !head || !dot) return;
  const left = dot.getBoundingClientRect().left - frise.getBoundingClientRect().left + dot.offsetWidth / 2 + frise.scrollLeft;
  if (instant) head.style.transition = "none";
  head.style.left = `${left}px`;
  if (instant) {
    head.getBoundingClientRect();
    head.style.transition = "";
  }
  frise.classList.add("ready");
}

function openPanel(desk) {
  return desk.querySelector(".phrase-unit:has(.phrase-pick:checked) .phrase-panel");
}

function refreshChronicle(desk) {
  const units = [...desk.querySelectorAll(".phrase-unit")];
  if (!units.length) return;
  const parts = [];
  for (let index = 0; index < units.length; index += 1) {
    const unit = units[index];
    const key = unit.querySelector(".phrase-pick")?.value || "";
    const labelField = unit.querySelector("input[name^='label_']");
    const label = (labelField?.value || unit.querySelector(".phrase-node-name")?.textContent || "").trim() || "Ce moment";
    if (index === 0) {
      parts.push(`${label} le jour du dossier.`);
      continue;
    }
    const gap = desk.querySelector(`input[name="ecart_${cssAttr(key)}"]`);
    const days = Number.parseInt(gap?.value ?? "", 10);
    if (!Number.isFinite(days)) return;
    parts.push(`${apres(days)}, ${lowerFirst(label)}.`);
  }
  const chronicle = desk.querySelector("#phrase-chronicle");
  if (chronicle) chronicle.textContent = parts.join(" ");
}

function apres(days) {
  if (days === 0) return "Le même jour";
  if (days === 1) return "Un jour après";
  if (days >= 2 && days <= 16) {
    const word = JOUR_MOTS[days - 2];
    return `${word.charAt(0).toUpperCase()}${word.slice(1)} jours après`;
  }
  return `${days} jours après`;
}

function lowerFirst(value) {
  if (!value) return value;
  return value.charAt(0).toLowerCase() + value.slice(1);
}

bootPhrases(document);

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
  document.body.addEventListener("griffe:saved", closePanel);
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

// Les travaux : le markdown se garde avant de quitter. Le rendu est l'autre page.
(() => {
  let travauxInflight = 0;
  let travauxXhr = null;
  let travauxFlushing = false;
  let travauxLetting = false;
  // Relire remplace #content avant htmx:afterRequest. L'événement repart
  // alors d'un ancêtre, et le formulaire n'est plus là. On suit le XHR.
  const travauxRequests = new Set();

  function travauxFormOf(event) {
    const elt = event.detail?.elt;
    if (!(elt instanceof Element)) return null;
    if (elt.matches("form.travaux-ecrire")) return elt;
    return elt.closest("form.travaux-ecrire");
  }

  function travauxForm() {
    return document.querySelector("form.travaux-ecrire");
  }

  function travauxDirty(form) {
    const area = form.querySelector("textarea.travaux-source");
    return Boolean(area) && area.value !== form.dataset.saved;
  }

  document.body.addEventListener("input", (event) => {
    const area = event.target;
    if (!(area instanceof HTMLTextAreaElement) || !area.classList.contains("travaux-source")) return;
    const etat = document.getElementById("travaux-etat");
    if (etat) etat.textContent = "";
  });

  function settleTravaux(event) {
    const xhr = event.detail?.xhr;
    if (!xhr || !travauxRequests.has(xhr)) return null;
    travauxRequests.delete(xhr);
    travauxInflight = Math.max(0, travauxInflight - 1);
    if (travauxXhr === xhr) travauxXhr = null;
    return xhr;
  }

  document.body.addEventListener("htmx:beforeRequest", (event) => {
    if (!travauxFormOf(event)) return;
    if (travauxFlushing) {
      event.preventDefault();
      return;
    }
    const xhr = event.detail?.xhr;
    if (!xhr || travauxRequests.has(xhr)) return;
    travauxRequests.add(xhr);
    travauxInflight += 1;
    travauxXhr = xhr;
  });

  document.body.addEventListener("htmx:afterRequest", (event) => {
    const xhr = settleTravaux(event);
    if (!xhr || !event.detail.successful) return;
    if (xhr.getResponseHeader("X-Griffe-Travaux") !== "ok") return;
    const form = travauxForm() || travauxFormOf(event);
    if (!form) return;
    const sent = event.detail.requestConfig?.parameters?.body;
    if (typeof sent === "string") form.dataset.saved = sent;
  });

  document.body.addEventListener("htmx:onLoadError", (event) => {
    settleTravaux(event);
  });

  function travauxIdle() {
    if (travauxInflight === 0) return Promise.resolve();
    return new Promise((resolve) => {
      const onDone = () => {
        if (travauxInflight > 0) return;
        document.body.removeEventListener("htmx:afterRequest", onDone);
        resolve();
      };
      document.body.addEventListener("htmx:afterRequest", onDone);
    });
  }

  function applyTravauxFragment(html) {
    const doc = new DOMParser().parseFromString(html, "text/html");
    ["travaux-revision", "travaux-etat", "travaux-alerte"].forEach((id) => {
      const next = doc.getElementById(id);
      const current = document.getElementById(id);
      if (!next || !current) return;
      current.replaceWith(next);
    });
    const alert = document.getElementById("travaux-alerte");
    if (alert && window.htmx) window.htmx.process(alert);
  }

  async function postTravaux(form, posted, revision) {
    const url = form.getAttribute("action");
    if (!url) return false;
    const body = new URLSearchParams();
    body.set("body", posted);
    body.set("revision", revision);
    let response;
    try {
      response = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/x-www-form-urlencoded",
          "HX-Request": "true",
        },
        body: body.toString(),
      });
    } catch {
      return false;
    }
    const html = await response.text();
    applyTravauxFragment(html);
    return response.headers.get("X-Griffe-Travaux") === "ok";
  }

  async function flushTravaux() {
    travauxFlushing = true;
    try {
      await travauxIdle();
      for (let guard = 0; guard < 4; guard += 1) {
        const form = travauxForm();
        if (!form) return true;
        const area = form.querySelector("textarea.travaux-source");
        const revision = form.querySelector("#travaux-revision");
        if (!area || area.value === form.dataset.saved) return true;
        const posted = area.value;
        const ok = await postTravaux(form, posted, revision ? revision.value : "");
        if (!ok) return false;
        form.dataset.saved = posted;
      }
      const form = travauxForm();
      return !form || !travauxDirty(form);
    } finally {
      travauxFlushing = false;
    }
  }

  document.addEventListener("click", (event) => {
    if (travauxLetting || event.defaultPrevented) return;
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const form = travauxForm();
    if (!form) return;
    if (travauxFlushing) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    const link = event.target instanceof Element ? event.target.closest("a[href]") : null;
    if (!link) return;
    if (link.hasAttribute("data-travaux-discard")) {
      const area = form.querySelector("textarea.travaux-source");
      if (area) form.dataset.saved = area.value;
      return;
    }
    if (!travauxDirty(form) && travauxInflight === 0) return;
    event.preventDefault();
    event.stopPropagation();
    flushTravaux().then((ok) => {
      if (!ok) return;
      travauxLetting = true;
      link.click();
      travauxLetting = false;
    });
  }, true);

  window.addEventListener("popstate", (event) => {
    const form = travauxForm();
    if (!form) return;
    const previous = window.onpopstate;
    window.onpopstate = null;
    event.stopPropagation();
    const dest = `${location.pathname}${location.search}`;
    const editUrl = form.dataset.edit || "";
    flushTravaux()
      .then((ok) => {
        if (!ok) {
          if (editUrl) history.pushState({ htmx: true }, "", editUrl);
          return undefined;
        }
        const content = document.getElementById("content");
        if (!content || !window.htmx) return undefined;
        return window.htmx.ajax("GET", dest, {
          source: content,
          target: content,
          swap: "innerHTML",
        });
      })
      .finally(() => {
        window.onpopstate = previous;
      });
  }, true);

  window.addEventListener("pagehide", () => {
    const form = travauxForm();
    if (!form || !travauxDirty(form)) return;
    const area = form.querySelector("textarea.travaux-source");
    const revision = form.querySelector("#travaux-revision");
    const url = form.getAttribute("action");
    if (!area || !url) return;
    if (travauxXhr) {
      travauxXhr.abort();
      travauxXhr = null;
    }
    const body = new URLSearchParams();
    body.set("body", area.value);
    body.set("revision", revision ? revision.value : "");
    fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "HX-Request": "true",
      },
      body: body.toString(),
      keepalive: true,
    }).catch(() => {});
  });
})();

// Les affaires, la lentille. Le dossier se charge dans #affaire ; la liste reste.
(() => {
  const FULL_KEY = "griffe-lens-full";

  function lensRoot() {
    return document.querySelector(".lens");
  }

  function lensField(node) {
    return node instanceof Element
      && (node.tagName === "INPUT"
        || node.tagName === "TEXTAREA"
        || node.tagName === "SELECT"
        || node.isContentEditable);
  }

  function paletteOpen() {
    const overlay = document.getElementById("palette-overlay");
    return Boolean(overlay && overlay.classList.contains("open"));
  }

  function affairePath(url) {
    if (!url || url.charAt(0) === "#") return false;
    let path = url;
    try {
      path = new URL(url, location.origin).pathname;
    } catch {
      return false;
    }
    if (path === "/affaires" || path === "/affaires/") return false;
    if (!path.startsWith("/affaires/")) return false;
    if (path === "/affaires/types" || path.startsWith("/affaires/types/")) return false;
    if (path === "/affaires/phrases" || path.startsWith("/affaires/phrases/")) return false;
    return true;
  }

  function retargetAffaire(node) {
    if (!(node instanceof Element) || !document.getElementById("affaire")) return;
    if (node.matches("form.travaux-ecrire, textarea.travaux-source")) return;
    if (node.getAttribute("hx-swap") === "none") return;
    const url = node.getAttribute("hx-get")
      || node.getAttribute("hx-post")
      || node.getAttribute("hx-delete")
      || node.getAttribute("action")
      || "";
    if (!affairePath(url)) return;
    const target = node.getAttribute("hx-target");
    if (target && target !== "#content") return;
    node.setAttribute("hx-target", "#affaire");
    if (!node.getAttribute("hx-swap")) node.setAttribute("hx-swap", "innerHTML");
  }

  function armLens() {
    const lens = lensRoot();
    if (!lens) return;
    let full = false;
    try {
      full = sessionStorage.getItem(FULL_KEY) === "1";
    } catch {
      full = false;
    }
    lens.classList.toggle("full", full);
    const span = lens.querySelector("[data-lens-span]");
    if (span) span.textContent = full ? "Réduire" : "Pleine largeur";
    const close = lens.querySelector(".lens-close");
    const list = lens.dataset.list || "/affaires";
    if (close) {
      close.setAttribute("href", list);
      close.setAttribute("hx-get", list);
    }
    lens.querySelectorAll(".trow").forEach((row) => {
      const href = row.getAttribute("href") || "";
      let open = false;
      try {
        open = new URL(href, location.origin).pathname === location.pathname;
      } catch {
        open = false;
      }
      row.classList.toggle("open", open);
    });
  }

  function personBack(lens) {
    const link = lens.querySelector(".lens-body a.back");
    if (!link) return null;
    const href = link.getAttribute("href") || "";
    return affairePath(href) ? link : null;
  }

  function closeLens(lens) {
    const close = lens.querySelector(".lens-close");
    if (close && lens.querySelector(".lens-bar")) close.click();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", armLens);
  } else {
    armLens();
  }
  document.body.addEventListener("htmx:afterSwap", armLens);

  document.addEventListener("click", (event) => {
    const lens = lensRoot();
    if (!lens || !(event.target instanceof Element)) return;
    const span = event.target.closest("[data-lens-span]");
    if (span && lens.contains(span)) {
      event.preventDefault();
      const full = !lens.classList.contains("full");
      lens.classList.toggle("full", full);
      try {
        sessionStorage.setItem(FULL_KEY, full ? "1" : "0");
      } catch {
        /* la largeur repart au prochain chargement */
      }
      span.textContent = full ? "Réduire" : "Pleine largeur";
      return;
    }
    const row = event.target.closest("a.trow");
    if (row && lens.contains(row)) {
      let same = false;
      try {
        same = new URL(row.getAttribute("href") || "", location.origin).pathname === location.pathname;
      } catch {
        same = false;
      }
      if (same && lens.querySelector(".lens-bar")) {
        event.preventDefault();
        event.stopPropagation();
        closeLens(lens);
        return;
      }
    }
    const table = event.target.closest(".lens-table, .lens-head");
    if (
      table
      && lens.contains(table)
      && lens.querySelector(".lens-bar")
      && !event.target.closest("a, button, input, summary, label, textarea, select")
    ) {
      event.preventDefault();
      closeLens(lens);
      return;
    }
    const node = event.target.closest("[hx-get], [hx-post], [hx-delete]");
    if (node) retargetAffaire(node);
  }, true);

  document.addEventListener("submit", (event) => {
    if (event.target instanceof Element) retargetAffaire(event.target);
  }, true);

  document.addEventListener("change", (event) => {
    const form = event.target instanceof Element ? event.target.closest("form") : null;
    if (form) retargetAffaire(form);
  }, true);

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    const lens = lensRoot();
    if (!lens) return;
    if (document.querySelector(".date-pick-pop:not([hidden])")) return;
    const active = document.activeElement;
    if (lensField(active) && lens.contains(active)) {
      active.blur();
      return;
    }
    const back = personBack(lens);
    if (back) {
      event.preventDefault();
      back.click();
      return;
    }
    if (lens.querySelector(".lens-bar")) {
      event.preventDefault();
      closeLens(lens);
      return;
    }
    lens.querySelectorAll(".trow.cur").forEach((row) => row.classList.remove("cur"));
  }, true);

  document.addEventListener("keydown", (event) => {
    const lens = lensRoot();
    if (!lens || event.metaKey || event.ctrlKey || event.altKey || paletteOpen()) return;
    if (lensField(event.target)) return;
    if (event.key === "/") {
      const search = document.getElementById("q");
      if (!search || !lens.contains(search)) return;
      event.preventDefault();
      search.focus();
      return;
    }
    if (event.key === "f" || event.key === "F") {
      const span = lens.querySelector("[data-lens-span]");
      if (!span) return;
      event.preventDefault();
      span.click();
      return;
    }
    if (event.key === "e" || event.key === "E") {
      const write = lens.querySelector('.lens-body a.seal[href$="/ecrire"]');
      if (!write) return;
      event.preventDefault();
      write.click();
      return;
    }
    const down = event.key === "j" || event.key === "J" || event.key === "ArrowDown";
    const up = event.key === "k" || event.key === "K" || event.key === "ArrowUp";
    if (down || up) {
      const rows = [...lens.querySelectorAll(".trow")];
      if (!rows.length) return;
      event.preventDefault();
      let index = rows.findIndex((row) => row.classList.contains("cur"));
      if (index < 0) index = rows.findIndex((row) => row.classList.contains("open"));
      if (index < 0) index = down ? -1 : 0;
      const next = Math.max(0, Math.min(rows.length - 1, index + (down ? 1 : -1)));
      rows.forEach((row) => row.classList.remove("cur"));
      rows[next].classList.add("cur");
      rows[next].scrollIntoView({ block: "nearest" });
      return;
    }
    if (event.key === "Enter") {
      const current = lens.querySelector(".trow.cur");
      if (!current) return;
      event.preventDefault();
      current.click();
    }
  });
})();
