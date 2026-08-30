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
