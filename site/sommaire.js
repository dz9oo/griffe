(() => {
  const links = [...document.querySelectorAll(".index a")];
  const sections = links
    .map((a) => document.getElementById(a.getAttribute("href").slice(1)))
    .filter(Boolean);
  const mark = (id) => {
    for (const a of links) {
      const here = a.getAttribute("href") === "#" + id;
      a.classList.toggle("is-here", here);
      if (here) a.setAttribute("aria-current", "location");
      else a.removeAttribute("aria-current");
    }
  };
  let ticking = false;
  const spy = () => {
    ticking = false;
    const y = 160;
    let current = sections[0] ? sections[0].id : "";
    for (const section of sections) {
      if (section.getBoundingClientRect().top <= y) current = section.id;
    }
    if (current) mark(current);
  };
  addEventListener("scroll", () => {
    if (!ticking) {
      ticking = true;
      requestAnimationFrame(spy);
    }
  }, { passive: true });
  spy();
})();
