// Injected before the page's own scripts (Playwright add_init_script). Stands in
// for the Tauri runtime a page reaches through @tauri-apps/api, so the front end
// runs in a plain browser. Commands answer from window.__MOCK_INVOKE__, a map of
// command name to a value or a function(args); anything unmapped answers null.
(() => {
	const callbacks = new Map();
	const listeners = new Map(); // event name -> handler ids, from plugin:event|listen
	let nextId = 1;
	const label = 'main';

	window.__TAURI_INTERNALS__ = {
		metadata: {
			currentWindow: { label },
			currentWebview: { windowLabel: label, label },
			windows: [{ label }],
			webviews: [{ windowLabel: label, label }]
		},
		transformCallback(cb) {
			const id = nextId++;
			callbacks.set(id, cb);
			return id;
		},
		unregisterCallback(id) {
			callbacks.delete(id);
		},
		convertFileSrc: (path) => path,
		async invoke(cmd, args = {}) {
			if (cmd === 'plugin:event|listen') {
				const ids = listeners.get(args.event) ?? [];
				ids.push(args.handler);
				listeners.set(args.event, ids);
				return args.handler;
			}
			if (cmd === 'plugin:event|unlisten') {
				for (const ids of listeners.values()) {
					const at = ids.indexOf(args.eventId);
					if (at >= 0) ids.splice(at, 1);
				}
				return null;
			}
			const mocked = (window.__MOCK_INVOKE__ ?? {})[cmd];
			return typeof mocked === 'function' ? mocked(args) : (mocked ?? null);
		}
	};
	window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

	// Fires an event at every handler the page registered for it.
	window.__tauriEmit = (event, payload) => {
		for (const id of listeners.get(event) ?? []) {
			callbacks.get(id)?.({ event, id, payload });
		}
	};
	window.__tauriListeners = () => [...listeners.keys()];
})();
