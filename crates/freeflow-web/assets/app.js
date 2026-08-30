// FreeFlow — script maison, volontairement minimal (pas de framework, pas de lib de charts :
// les graphes sont du SVG rendu côté serveur). Deux responsabilités : mettre à jour l'onglet
// actif après une navigation htmx boostée, et piloter la palette de commandes ⌘K. Le rail
// d'audit se rafraîchit tout seul via un polling htmx déclaratif (voir layout.rs) — pas besoin
// de JS pour ça.

document.body.addEventListener("click", (event) => {
  const tab = event.target.closest(".tab[data-view]");
  if (!tab) return;
  document.querySelectorAll(".tab[data-view]").forEach((t) => t.classList.remove("active"));
  tab.classList.add("active");
});

const paletteOverlay = document.getElementById("palette-overlay");
const paletteInput = document.getElementById("palette-input");

function closePalette() {
  paletteOverlay.classList.remove("open");
  paletteInput.value = "";
  filterPalette();
}

function openPalette() {
  paletteOverlay.classList.add("open");
  paletteInput.focus();
}

function filterPalette() {
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

const consoleLog = document.getElementById("console-log");
if (consoleLog) {
  document.body.addEventListener("htmx:afterSwap", (event) => {
    if (event.target.id === "console-log") {
      consoleLog.scrollTop = consoleLog.scrollHeight;
    }
  });
}
