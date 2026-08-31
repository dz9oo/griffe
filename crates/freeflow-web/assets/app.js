// FreeFlow — script maison, volontairement minimal (pas de framework, pas de lib de charts :
// les graphes sont du SVG rendu côté serveur). Trois responsabilités : mettre à jour l'onglet
// actif après une navigation htmx boostée, piloter la palette de commandes ⌘K, et signaler une
// activité humaine réelle au serveur pour l'auto-verrouillage (voir state.rs) — sans quoi le
// polling du rail d'audit, qui n'est pas de l'activité humaine, ne prolongerait jamais rien de
// lui-même. Cette page n'est chargée que par la coque applicative (jamais par l'écran de
// déverrouillage, volontairement dépourvu de palette et de rail) : tout ici suppose que ces
// éléments existent, mais reste défensif au cas où un futur écran partiel ne les inclurait pas.

document.body.addEventListener("click", (event) => {
  const tab = event.target.closest(".tab[data-view]");
  if (!tab) return;
  document.querySelectorAll(".tab[data-view]").forEach((t) => t.classList.remove("active"));
  tab.classList.add("active");
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
  // Remplace l'ancien `onclick=` inline, de toute façon inerte : la CSP (`script-src 'self'`,
  // pas de `'unsafe-inline'`) bloque les gestionnaires d'événements en attribut HTML.
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

// Reset + refocus le champ de la console après chaque soumission — remplace l'attribut htmx
// `hx-on--after-request`, qui s'appuie sur `new Function` et est donc bloqué par la CSP de la
// fenêtre packagée (`script-src 'self'`, sans `'unsafe-eval'` — voir `views/console.rs`).
document.body.addEventListener("htmx:afterRequest", (event) => {
  if (event.target.id === "console-form") {
    event.target.reset();
    event.target.querySelector("input")?.focus();
  }
});

// Panneau latéral (`#panel`) : ouvert par un `hx-get`/`hx-post` ciblé dessus depuis un écran de
// données (voir `views/clients.rs`), fermé côté client uniquement — aucune de ces trois actions
// ne fait de round-trip serveur.
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
  // Après une mutation réussie (`HX-Trigger: freeflow:saved`, voir `crate::clients`), le corps
  // de la réponse est vide : htmx vide déjà #panel par le swap lui-même. Cet écouteur reste un
  // filet pour toute réponse qui déclencherait l'événement sans passer par ce swap.
  document.body.addEventListener("freeflow:saved", closePanel);
}

// `n` ouvre l'action de création de l'écran actif, quand un panneau de données existe pour cet
// écran — étendu au fil des lots suivants (`NEW_ACTION_BY_VIEW` reste la seule chose à
// compléter). Inactif pendant la saisie d'un champ, ou pendant que la palette est ouverte.
const NEW_ACTION_BY_VIEW = { clients: "/clients/new" };
document.addEventListener("keydown", (event) => {
  if (event.key !== "n" || event.metaKey || event.ctrlKey || event.altKey) return;
  const target = event.target;
  if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT") return;
  if (paletteOverlay?.classList.contains("open")) return;
  const activeView = document.querySelector(".tab.active")?.dataset.view;
  const action = activeView && NEW_ACTION_BY_VIEW[activeView];
  if (!action || !panel) return;
  event.preventDefault();
  htmx.ajax("GET", action, { target: "#panel", swap: "innerHTML" });
});

// Auto-verrouillage sans démon : le rail d'audit poll `/audit/recent` toutes les 2s mais est
// explicitement exclu du calcul d'activité côté serveur (voir state.rs) — sans quoi la session
// n'expirerait jamais. Une vraie frappe ou un vrai clic prolonge la session, au plus une fois
// par minute pour ne pas transformer chaque geste en requête réseau.
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
