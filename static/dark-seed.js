// Dark-first seed (the `dark_first` copier answer; see app.html). Same-origin and render-blocking so
// the CSP needs no inline allowance. Seeds mode-watcher's key and class only when no preference is stored.
(function () {
	try {
		var key = 'mode-watcher-mode';
		if (!localStorage.getItem(key)) localStorage.setItem(key, 'dark');
		if (localStorage.getItem(key) === 'dark') document.documentElement.classList.add('dark');
	} catch {}
})();
