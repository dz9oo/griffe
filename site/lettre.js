(() => {
  const sheets = [...document.querySelectorAll(".sheet")];
  const matin = document.getElementById("matin");
  const deskNav = document.querySelector(".nav-desk");
  const matinNav = document.querySelector(".nav-matin");
  const links = new Map(
    [...matinNav.querySelectorAll("a[href^='#h']")].map((a) => [
      a.getAttribute("href").slice(1),
      a,
    ]),
  );
  const line = 120;
  const closeAt = 72;
  const openAt = 168;
  let letterOpen = false;
  let reading = false;

  const hashId = () => location.hash.slice(1);

  const syncLetter = () => {
    const id = hashId();
    if (id === "sceau" || links.has(id)) letterOpen = true;
    else if (id === "accueil" || id === "") letterOpen = false;
    document.body.classList.toggle("is-open", letterOpen);
  };

  const mark = (id) => {
    for (const [sheetId, link] of links) {
      const here = sheetId === id;
      link.classList.toggle("is-here", here);
      if (here) link.setAttribute("aria-current", "location");
      else link.removeAttribute("aria-current");
    }
  };

  const setReading = (next) => {
    reading = next;
    document.body.classList.toggle("is-reading", reading);
    deskNav.inert = reading;
    matinNav.inert = !reading;
    deskNav.setAttribute("aria-hidden", reading ? "true" : "false");
    matinNav.setAttribute("aria-hidden", reading ? "false" : "true");
  };

  let frame = 0;
  const spy = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      syncLetter();
      if (!letterOpen) {
        setReading(false);
        mark(null);
        return;
      }
      const bottom = matin.getBoundingClientRect().bottom;
      if (reading) {
        if (bottom < closeAt) setReading(false);
      } else if (bottom > openAt) {
        setReading(true);
      }
      if (!reading) {
        mark(null);
        return;
      }
      let current = null;
      for (const sheet of sheets) {
        const box = sheet.getBoundingClientRect();
        if (box.top <= line && box.bottom > line) current = sheet.id;
      }
      if (!current) {
        const first = sheets[0].getBoundingClientRect();
        const last = sheets[sheets.length - 1].getBoundingClientRect();
        if (first.top > line) current = sheets[0].id;
        else if (last.bottom <= line) current = sheets[sheets.length - 1].id;
      }
      mark(current);
    });
  };

  document.addEventListener("scroll", spy, { passive: true });
  window.addEventListener("hashchange", spy);
  window.addEventListener("resize", spy);
  window.addEventListener("load", spy);
  spy();
})();
