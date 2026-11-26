/**
 * The jsdom harness contract, asserted directly.
 *
 * `setup.ts` fills in the browser APIs jsdom does not implement. Nothing else
 * asserted they were there, and on 2026-08-19 a convergence dropped the Pointer
 * Capture shim and left a ResizeObserver mock that could not be `new`ed. The
 * only symptom, in an app whose CI then sat red for hours, was five component
 * tests failing with `Unable to find role="option"` — a portalled listbox that
 * never rendered, which reads as a broken component, not as a missing shim.
 *
 * These tests exist so that failure arrives here instead, naming the API that
 * went missing. They assert the SHAPE the consuming libraries actually call —
 * a constructor where a constructor is used — not merely that a global exists.
 */
import { describe, expect, it } from 'vitest';

describe('jsdom harness shims', () => {
	// bits-ui's Select trigger calls these in its own pointerdown handler,
	// before it opens the listbox. jsdom implements none of the Pointer Capture
	// API, so without them the handler throws and the popup never mounts.
	it('implements the Pointer Capture API on Element', () => {
		const element = document.createElement('button');

		expect(element.hasPointerCapture(1)).toBe(false);
		expect(() => element.setPointerCapture(1)).not.toThrow();
		expect(() => element.releasePointerCapture(1)).not.toThrow();
	});

	// floating-ui — under every popover, dropdown and Select in the UI kit —
	// calls `new ResizeObserver(...)` while positioning its layer. A mock that
	// only answers a plain call is not a constructor and throws there.
	it('provides a constructible ResizeObserver', () => {
		const observer = new ResizeObserver(() => {});

		expect(() => observer.observe(document.body)).not.toThrow();
		expect(() => observer.unobserve(document.body)).not.toThrow();
		expect(() => observer.disconnect()).not.toThrow();
	});

	it('provides a constructible IntersectionObserver', () => {
		const observer = new IntersectionObserver(() => {});

		expect(() => observer.observe(document.body)).not.toThrow();
		expect(observer.takeRecords()).toEqual([]);
		expect(() => observer.disconnect()).not.toThrow();
	});

	it('stubs the layout and animation APIs jsdom leaves unimplemented', () => {
		const element = document.createElement('div');

		expect(() => element.scrollIntoView()).not.toThrow();
		expect(element.animate([], {})).toMatchObject({ finished: expect.any(Promise) });
		expect(matchMedia('(min-width: 640px)').matches).toBe(false);
	});

	it('stubs localStorage', () => {
		localStorage.setItem('harness', 'present');

		expect(localStorage.getItem('harness')).toBe('present');
		expect(localStorage.length).toBe(1);

		localStorage.clear();
		expect(localStorage.getItem('harness')).toBeNull();
	});
});
