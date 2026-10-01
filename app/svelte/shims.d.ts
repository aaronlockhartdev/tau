// Makes .svelte component imports typed (ComponentType) for the TS language
// service, which cannot parse .svelte modules on its own: without this shim
// every imported component is error-typed and the type-aware no-unsafe-*
// lint rules report on each use. Svelte 5's documented TS shim.
declare module '*.svelte' {
  import type { ComponentType } from 'svelte';
  const component: ComponentType;
  export default component;
}
