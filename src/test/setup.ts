import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach, vi } from 'vitest';

afterEach(async () => {
	cleanup();

	// bits-ui's body-scroll-lock releases 24ms AFTER its last lock goes, so
	// unmounting a Dialog leaves a timer behind. Let it fire while jsdom is still
	// alive: once the environment is torn down the same callback reaches for
	// `document` and vitest reports an unhandled `ReferenceError` — attributed to
	// whichever file happened to be running rather than the one that opened the
	// dialog, which is why it names an innocent test. It had Frontend CI red on
	// main while the suite passed on every workstation: the timer wins the race
	// on a fast machine and loses it on a loaded runner.
	//
	// Gated on the lock's own marker, so the tests that never open an overlay pay
	// nothing for it: `overflow: hidden` on <body> is what the lock sets and what
	// its deferred cleanup takes away.
	if (document.body.style.overflow === 'hidden') {
		await new Promise((resolve) => setTimeout(resolve, 40));
	}
});

// Mock localStorage
const localStorageMock = (() => {
	let store: Record<string, string> = {};
	return {
		getItem: (key: string) => store[key] ?? null,
		setItem: (key: string, value: string) => {
			store[key] = value;
		},
		removeItem: (key: string) => {
			delete store[key];
		},
		clear: () => {
			store = {};
		},
		get length() {
			return Object.keys(store).length;
		},
		key: (index: number) => Object.keys(store)[index] ?? null
	};
})();
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock });

// Mock window.matchMedia
Object.defineProperty(globalThis, 'matchMedia', {
	value: vi.fn().mockImplementation((query: string) => ({
		matches: false,
		media: query,
		onchange: null,
		addListener: vi.fn(),
		removeListener: vi.fn(),
		addEventListener: vi.fn(),
		removeEventListener: vi.fn(),
		dispatchEvent: vi.fn()
	}))
});

// Mock ResizeObserver and IntersectionObserver. Both must be CONSTRUCTIBLE:
// floating-ui — which every popover, dropdown and Select in the UI kit sits on
// — calls `new ResizeObserver(...)` while positioning its layer, and an arrow
// function returning an object is not a constructor. A mock that only answers
// `ResizeObserver()` throws there instead, and the failure surfaces a long way
// from here as a portalled listbox that never renders.
//
// Deliberately NOT `implements ResizeObserver` / `implements IntersectionObserver`:
// the DOM lib grows members (`scrollMargin` arrived and broke two apps that were
// a TypeScript version ahead), and a mock that promises the whole interface has
// to chase every one. What the runtime needs is `new`-ability and the three
// methods the observers are actually called with, so the cast at assignment is
// the honest boundary rather than a widening lie in the class body.
class MockResizeObserver {
	observe = vi.fn();
	unobserve = vi.fn();
	disconnect = vi.fn();
}
globalThis.ResizeObserver = MockResizeObserver as unknown as typeof ResizeObserver;

class MockIntersectionObserver {
	readonly root = null;
	readonly rootMargin = '';
	readonly thresholds: readonly number[] = [];
	observe = vi.fn();
	unobserve = vi.fn();
	disconnect = vi.fn();
	takeRecords = vi.fn(() => []);
}
globalThis.IntersectionObserver =
	MockIntersectionObserver as unknown as typeof IntersectionObserver;

// Mock Element.scrollIntoView
Element.prototype.scrollIntoView = vi.fn();

// Mock the Pointer Capture API. jsdom implements none of it, and the bits-ui
// Select trigger calls hasPointerCapture() in its own pointerdown handler
// before it opens the listbox — so without these the popup never renders and
// every query for one of its options fails as though the component were broken.
Element.prototype.hasPointerCapture = vi.fn().mockReturnValue(false);
Element.prototype.setPointerCapture = vi.fn();
Element.prototype.releasePointerCapture = vi.fn();

// Mock Element.animate
Element.prototype.animate = vi.fn().mockReturnValue({
	cancel: vi.fn(),
	finished: Promise.resolve(),
	onfinish: null,
	oncancel: null
});
