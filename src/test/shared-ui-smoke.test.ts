// Interaction proof, not a render-only check (rules-library/core/73-verification.md
// §"Behaviour vs Appearance"): asserts the migrated @poodle64/ui components
// genuinely respond to a click, rather than only rendering. Covers the two
// shapes WP-125's migration touched most — a simple bindable toggle
// (Checkbox) and a portal-based compound component (Dialog) — plus the three
// state surfaces the canonical-shape alignment converged onto the package.
//
// The three are imported through `#lib/components/common`, the barrel that replaced
// the local copies: a test importing straight from @poodle64/ui would pass even
// if the barrel were broken, which is the only thing the swap actually changed.
import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import SharedUiSmoke from './shared-ui-smoke.svelte';

describe('@poodle64/ui consumption (WP-125 migration)', () => {
  it('toggles the migrated Checkbox on click', async () => {
    render(SharedUiSmoke);

    const checkbox = screen.getByRole('checkbox', { name: 'smoke-test-checkbox' });
    expect(checkbox).toHaveAttribute('aria-checked', 'false');

    await fireEvent.click(checkbox);

    await waitFor(() => {
      expect(checkbox).toHaveAttribute('aria-checked', 'true');
    });
  });

  it('opens the migrated Dialog on trigger click and shows its content', async () => {
    render(SharedUiSmoke);

    expect(screen.queryByText('Smoke test dialog')).not.toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: 'Open dialog' }));

    await waitFor(() => {
      expect(screen.getByText('Smoke test dialog')).toBeInTheDocument();
    });
  });
});

describe('converged state surfaces (canonical-shape alignment)', () => {
  it('renders EmptyState content and drives its action snippet', async () => {
    render(SharedUiSmoke);

    expect(screen.getByText('No transcriptions yet')).toBeInTheDocument();
    expect(screen.getByText('Press the hotkey to dictate.')).toBeInTheDocument();

    const clicks = screen.getByTestId('empty-action-clicks');
    expect(clicks).toHaveTextContent('0');

    await fireEvent.click(screen.getByRole('button', { name: 'Start recording' }));

    await waitFor(() => expect(clicks).toHaveTextContent('1'));
  });

  it('renders ErrorState content and drives its action snippet', async () => {
    render(SharedUiSmoke);

    expect(screen.getByText('Could not reach the model')).toBeInTheDocument();

    const clicks = screen.getByTestId('error-action-clicks');
    expect(clicks).toHaveTextContent('0');

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(clicks).toHaveTextContent('1'));
  });

  it('exposes LoadingState as a live status region that tracks its message', async () => {
    render(SharedUiSmoke);

    // role=status + aria-live is the whole contract: a spinner nothing announces
    // is a spinner a screen-reader user never learns about.
    const status = screen.getByRole('status', { name: 'Loading dictionary…' });
    expect(status).toHaveAttribute('aria-live', 'polite');

    // Observed at two instants, so this proves the region is reactive rather
    // than frozen at its initial value.
    await fireEvent.click(screen.getByRole('button', { name: 'Change loading message' }));

    await waitFor(() =>
      expect(screen.getByRole('status', { name: 'Calculating storage usage…' })).toBeInTheDocument()
    );
  });
});
