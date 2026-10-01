import '@testing-library/jest-dom/vitest';

// jsdom gaps the components touch:
// - Element.prototype.scrollIntoView (the composer scrolls the selected
//   dropdown option into view)
// - ResizeObserver (unused by the mocked virtualizer, but cheap insurance
//   for any future DOM-measuring code)
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
if (!('ResizeObserver' in globalThis)) {
  class RO {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = RO;
}
