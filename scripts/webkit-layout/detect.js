// Layout detector for sweep.py. Evaluated in the page; returns findings.
//
// A "leaf" is what a person sees: a text node's line boxes, or an atomic control
// or picture (button, input, select, img, svg, canvas, video, [role=button|slider]).
// Leaf boxes are the painted extent, not the element box, so text that spills out
// of a shrunken flex item is still seen where it lands.
//
//   overlap    leaves of two in-flow siblings of one container intersect
//   overflow   a leaf leaves its container's box, or the window, with nothing scrolling it
//   clipping   an overflow:hidden ancestor cuts a leaf and does not ellipsise it
//
// Opt a deliberate overlay out with data-sweep-ignore, or app-wide with the CSS
// selector list the app's IGNORE hands sweep.py (window.__SWEEP_IGNORE__).
(() => {
  const TOL = 2;
  const ATOMIC = new Set(['BUTTON', 'INPUT', 'SELECT', 'TEXTAREA', 'IMG', 'SVG', 'CANVAS', 'VIDEO']);
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'META', 'LINK', 'TITLE', 'PATH', 'DEFS']);
  // The app's UI zoom is CSS zoom on the root: the window is innerWidth px but
  // the layout, and every rect, is in zoomed px.
  const zoom = parseFloat(getComputedStyle(document.documentElement).zoom) || 1;
  const vw = window.innerWidth / zoom;
  const vh = window.innerHeight / zoom;
  const pageScrollsY = document.documentElement.scrollHeight > window.innerHeight + 1;

  const styles = new Map();
  const style = (el) => {
    let s = styles.get(el);
    if (!s) styles.set(el, (s = getComputedStyle(el)));
    return s;
  };
  const box = (el) => {
    const r = el.getBoundingClientRect();
    return { l: r.left, t: r.top, r: r.right, b: r.bottom };
  };
  const fromRect = (r) => ({ l: r.left, t: r.top, r: r.right, b: r.bottom });
  const inter = (a, b) => ({
    l: Math.max(a.l, b.l), t: Math.max(a.t, b.t), r: Math.min(a.r, b.r), b: Math.min(a.b, b.b),
  });
  const empty = (r) => r.r - r.l <= 0 || r.b - r.t <= 0;
  const union = (rs) => rs.reduce((u, r) => ({
    l: Math.min(u.l, r.l), t: Math.min(u.t, r.t), r: Math.max(u.r, r.r), b: Math.max(u.b, r.b),
  }));

  const sel = (el) => {
    let s = el.tagName.toLowerCase();
    if (el.id) s += '#' + el.id;
    const cls = [...el.classList].filter((c) => !/^svelte-/.test(c)).slice(0, 3);
    return cls.length ? s + '.' + cls.join('.') : s;
  };
  const path = (el) => {
    const parts = [];
    for (let e = el, i = 0; e && e !== document.body && i < 3; e = e.parentElement, i++) parts.unshift(sel(e));
    return parts.join(' > ');
  };
  const snippet = (leaf) => {
    const t = (leaf.el.innerText || leaf.el.getAttribute('aria-label') || leaf.el.getAttribute('title') || leaf.el.alt || '').trim();
    return t.replace(/\s+/g, ' ').slice(0, 40);
  };

  const clipMemo = new Map();
  const viewport = { l: 0, t: 0, r: vw, b: vh };
  const clipOf = (el) => {
    if (el === document.documentElement || !el) return viewport;
    let c = clipMemo.get(el);
    if (c) return c;
    const s = style(el);
    c = s.position === 'fixed' ? viewport : clipOf(el.parentElement);
    if (s.overflowX !== 'visible' || s.overflowY !== 'visible') {
      const b = box(el);
      c = {
        l: s.overflowX !== 'visible' ? Math.max(c.l, b.l) : c.l,
        r: s.overflowX !== 'visible' ? Math.min(c.r, b.r) : c.r,
        t: s.overflowY !== 'visible' ? Math.max(c.t, b.t) : c.t,
        b: s.overflowY !== 'visible' ? Math.min(c.b, b.b) : c.b,
      };
    }
    clipMemo.set(el, c);
    return c;
  };

  const opacityMemo = new Map();
  const opacityOf = (el) => {
    if (!el || el === document.documentElement) return 1;
    let o = opacityMemo.get(el);
    if (o === undefined) {
      o = parseFloat(style(el).opacity) * opacityOf(el.parentElement);
      opacityMemo.set(el, o);
    }
    return o;
  };

  const IGNORE = ['[data-sweep-ignore]', '[inert]', window.__SWEEP_IGNORE__].filter(Boolean).join(', ');
  const ignored = (el) => !!el.closest(IGNORE);
  const isAtomic = (el) =>
    ATOMIC.has(el.tagName.toUpperCase()) || ['button', 'slider'].includes(el.getAttribute('role'));

  const findings = [];
  const elements = [];
  const report = (f, els) => {
    findings.push(f);
    elements.push(els);
  };

  const leafFor = (el, owner, rects, kind) => {
    const clipped = [];
    for (const r of rects) {
      if (r.r - r.l < 1 || r.b - r.t < 1) continue;
      const c = inter(r, clipOf(owner));
      if (!empty(c)) clipped.push(c);
    }
    if (!clipped.length) return null;
    const all = rects.filter((r) => r.r - r.l >= 1 && r.b - r.t >= 1);
    if (all.length === 0) return null;
    const bb = union(all);
    if (bb.r - bb.l <= 2 && bb.b - bb.t <= 2) return null;
    return { el, owner, kind, rects: clipped, bbox: bb };
  };

  function visit(el) {
    if (SKIP.has(el.tagName.toUpperCase()) || ignored(el)) return [];
    const s = style(el);
    if (s.display === 'none' || s.visibility === 'hidden' || opacityOf(el) < 0.05) return [];
    if (isAtomic(el)) {
      const r = box(el);
      const l = leafFor(el, el, [r], el.tagName === 'IMG' || el.tagName === 'svg' ? 'image' : 'control');
      return l ? [l] : [];
    }
    const own = [];
    const kids = [];
    for (const n of el.childNodes) {
      if (n.nodeType === 3) {
        if (!n.nodeValue.trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(n);
        const l = leafFor(el, el, [...range.getClientRects()].map(fromRect), 'text');
        if (l) own.push(l);
      } else if (n.nodeType === 1) {
        const leaves = visit(n);
        if (leaves.length) kids.push({ el: n, leaves });
      }
    }
    const flow = kids.filter((k) => {
      const ks = style(k.el);
      return !['absolute', 'fixed'].includes(ks.position) && !['inline', 'contents'].includes(ks.display);
    });
    for (let i = 0; i < flow.length; i++) {
      for (let j = i + 1; j < flow.length; j++) {
        const hit = firstOverlap(flow[i].leaves, flow[j].leaves);
        if (!hit) continue;
        report(
          {
            kind: 'overlap',
            container: path(el),
            a: { sel: path(hit.a.el), text: snippet(hit.a) },
            b: { sel: path(hit.b.el), text: snippet(hit.b) },
            detail: `${Math.round(hit.w)}x${Math.round(hit.h)}px`,
          },
          [hit.a.el, hit.b.el]
        );
      }
    }
    return own.concat(...kids.map((k) => k.leaves));
  }

  function firstOverlap(as, bs) {
    for (const a of as) {
      for (const b of bs) {
        for (const ra of a.rects) {
          for (const rb of b.rects) {
            const i = inter(ra, rb);
            const w = i.r - i.l;
            const h = i.b - i.t;
            if (w <= TOL || h <= TOL) continue;
            const minH = Math.min(ra.b - ra.t, rb.b - rb.t);
            if (h < 0.25 * minH) continue;
            return { a, b, w, h };
          }
        }
      }
    }
    return null;
  }

  const ellipsisBetween = (from, to, axis) => {
    for (let e = from; e; e = e.parentElement) {
      const s = style(e);
      if (axis === 'x' && s.textOverflow === 'ellipsis') return true;
      if (axis === 'y') {
        const v = s.getPropertyValue('-webkit-line-clamp');
        if (v && v !== 'none') return true;
      }
      if (e === to) break;
    }
    return false;
  };

  const containerReported = new Set();
  function analyse(leaf) {
    const bb = leaf.bbox;
    const start = leaf.kind === 'text' ? leaf.el : leaf.el.parentElement;
    let xStop = false;
    let yStop = false;
    for (let a = start; a && a !== document.documentElement; a = a.parentElement) {
      const s = style(a);
      const b = box(a);
      if (!xStop) {
        if (s.overflowX === 'auto' || s.overflowX === 'scroll') xStop = true;
        else if (s.overflowX !== 'visible') {
          const cut = Math.max(b.l - bb.l, bb.r - b.r);
          const touches = bb.r > b.l && bb.l < b.r;
          if (leaf.kind !== 'image' && touches && cut > TOL && !(leaf.kind === 'text' && ellipsisBetween(leaf.el, a, 'x'))) {
            report(
              { kind: 'clipping', container: path(a), a: { sel: path(leaf.el), text: snippet(leaf) }, detail: `cut ${Math.round(cut)}px horizontally` },
              [leaf.el, a]
            );
          }
          xStop = true;
        }
      }
      if (!yStop) {
        if (s.overflowY === 'auto' || s.overflowY === 'scroll') yStop = true;
        else if (s.overflowY !== 'visible') {
          const cut = Math.max(b.t - bb.t, bb.b - b.b);
          const touches = bb.b > b.t && bb.t < b.b;
          if (leaf.kind !== 'image' && touches && cut > 3 && !(leaf.kind === 'text' && ellipsisBetween(leaf.el, a, 'y'))) {
            report(
              { kind: 'clipping', container: path(a), a: { sel: path(leaf.el), text: snippet(leaf) }, detail: `cut ${Math.round(cut)}px vertically` },
              [leaf.el, a]
            );
          }
          yStop = true;
        }
      }
      if (xStop && yStop) break;
    }
    if (!xStop && (bb.r > vw + TOL || bb.l < -TOL)) {
      report(
        { kind: 'overflow', container: 'window', a: { sel: path(leaf.el), text: snippet(leaf) }, detail: `${Math.round(bb.r - vw)}px past the right edge` },
        [leaf.el]
      );
    }
    if (!yStop && !pageScrollsY && bb.b > vh + TOL) {
      report(
        { kind: 'overflow', container: 'window', a: { sel: path(leaf.el), text: snippet(leaf) }, detail: `${Math.round(bb.b - vh)}px below the window` },
        [leaf.el]
      );
    }
    let p = start;
    while (p && ['inline', 'contents'].includes(style(p).display)) p = p.parentElement;
    if (p && p !== document.body && p !== document.documentElement && style(p).overflowX === 'visible' && !containerReported.has(p)) {
      const b = box(p);
      if (bb.r > b.r + TOL || bb.l < b.l - TOL) {
        containerReported.add(p);
        report(
          { kind: 'overflow', container: path(p), a: { sel: path(leaf.el), text: snippet(leaf) }, detail: `${Math.round(Math.max(bb.r - b.r, b.l - bb.l))}px outside its container` },
          [leaf.el, p]
        );
      }
    }
  }

  const leaves = visit(document.body);
  for (const l of leaves) analyse(l);

  if (document.documentElement.scrollWidth > window.innerWidth + 1) {
    report(
      { kind: 'overflow', container: 'window', a: { sel: 'html', text: '' }, detail: `page scrolls horizontally by ${document.documentElement.scrollWidth - window.innerWidth}px` },
      [document.documentElement]
    );
  }

  window.__sweepEls = elements;
  window.__sweepMark = () => {
    elements.forEach((els, i) => els.forEach((el, k) => {
      el.style.outline = `2px solid ${k === 0 ? '#ff2d55' : '#ffcc00'}`;
      el.style.outlineOffset = '-1px';
    }));
  };
  return findings;
})()
