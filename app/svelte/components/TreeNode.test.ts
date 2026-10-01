// @vitest-environment jsdom
// The shared row shell: indent, chevron states, class tints, and the
// keyboard/click routing (row click, dblclick, chevron toggle, Enter).
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import TreeNodeFixture from '../lib/testing/TreeNodeFixture.svelte';

describe('TreeNode', () => {
  it('renders the label snippet', () => {
    render(TreeNodeFixture, { props: { text: 'alpha' } });
    expect(screen.getByText('alpha')).toBeInTheDocument();
  });

  it('has no chevron when expanded is null', () => {
    render(TreeNodeFixture, { props: { text: 'leaf' } });
    const chev = document.querySelector('.chev')!;
    expect(chev).toBeDisabled();
    expect(chev.querySelector('svg')).toBeNull();
  });

  it('chevron reads open when expanded is true', () => {
    render(TreeNodeFixture, { props: { text: 'open', expanded: true } });
    expect(document.querySelector('.chev')).toHaveClass('open');
    expect(document.querySelector('.chev')).toHaveClass('has');
  });

  it('chevron is closed (but present) when expanded is false', () => {
    render(TreeNodeFixture, { props: { text: 'closed', expanded: false } });
    const chev = document.querySelector('.chev')!;
    expect(chev).not.toHaveClass('open');
    expect(chev.querySelector('svg')).toBeTruthy();
  });

  it('applies the sel / multi / dimmed tints', () => {
    render(TreeNodeFixture, { props: { text: 'x', selected: true, multi: true, dimmed: true } });
    const row = document.querySelector('.trow')!;
    expect(row).toHaveClass('sel');
    expect(row).toHaveClass('multi');
    expect(row).toHaveClass('dimmed');
  });

  it('depth drives the indent custom property', () => {
    render(TreeNodeFixture, { props: { text: 'deep', depth: 3 } });
    expect((document.querySelector('.node') as HTMLElement).style.getPropertyValue('--indent')).toBe('30px');
  });

  it('a row click calls onRow with the mouse event', async () => {
    const onRow = vi.fn();
    const user = userEvent.setup();
    render(TreeNodeFixture, { props: { text: 'a', onRow } });
    await user.click(screen.getByText('a'));
    expect(onRow).toHaveBeenCalledTimes(1);
    expect(onRow.mock.calls[0]![0]).toBeInstanceOf(MouseEvent);
  });

  it('a double click calls onRowDbl', async () => {
    const onRowDbl = vi.fn();
    const user = userEvent.setup();
    render(TreeNodeFixture, { props: { text: 'a', onRowDbl } });
    await user.dblClick(screen.getByText('a'));
    expect(onRowDbl).toHaveBeenCalledTimes(1);
  });

  it('a dimmed row drops the double click', async () => {
    const onRowDbl = vi.fn();
    const user = userEvent.setup();
    render(TreeNodeFixture, { props: { text: 'a', dimmed: true, onRowDbl } });
    await user.dblClick(screen.getByText('a'));
    expect(onRowDbl).not.toHaveBeenCalled();
  });

  it('the chevron calls onToggle and swallows the click (row untouched)', async () => {
    const onRow = vi.fn();
    const onToggle = vi.fn();
    const user = userEvent.setup();
    render(TreeNodeFixture, { props: { text: 'a', expanded: true, onRow, onToggle } });
    await user.click(document.querySelector('.chev')!);
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onRow).not.toHaveBeenCalled();
  });

  it('Enter routes to onRow when present', async () => {
    const onRow = vi.fn();
    render(TreeNodeFixture, { props: { text: 'a', onRow } });
    await fireEvent.keyDown(document.querySelector('.trow')!, { key: 'Enter' });
    expect(onRow).toHaveBeenCalledTimes(1);
  });

  it('Enter routes to onToggle when there is no onRow', async () => {
    const onToggle = vi.fn();
    render(TreeNodeFixture, { props: { text: 'a', expanded: false, onToggle } });
    await fireEvent.keyDown(document.querySelector('.trow')!, { key: ' ' });
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it('a context menu calls onContext', async () => {
    const onContext = vi.fn();
    render(TreeNodeFixture, { props: { text: 'a', onContext } });
    await fireEvent.contextMenu(document.querySelector('.trow')!);
    expect(onContext).toHaveBeenCalledTimes(1);
  });
});
