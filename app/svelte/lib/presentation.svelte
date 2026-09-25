<script module lang="ts">
  // The presentation registry (spec §12, #35): in-app, per-user — how the
  // GUI renders. Deliberately NOT the layered config file: a project file
  // must not dictate display preferences. The webview's localStorage is the
  // per-user store (no new dependency; the plain-browser dev rig shares it).
  const KEY = 'tau.presentation';

  export interface PresentationPrefs {
    // Reasoning ("thinking") lines open by default (spec §6).
    reasoningVisibleByDefault: boolean;
    // Hide thinking blocks entirely.
    hideThinking: boolean;
    // Surface a notice when a cached request's prompt cache misses.
    showCacheMiss: boolean;
  }

  const DEFAULTS: PresentationPrefs = {
    reasoningVisibleByDefault: true,
    hideThinking: false,
    showCacheMiss: true
  };

  function load(): PresentationPrefs {
    try {
      const raw = localStorage.getItem(KEY);
      if (!raw) return { ...DEFAULTS };
      // Merge over the defaults: a file written by an older build lacks the
      // new keys, and a partial write must not reset the others.
      return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<PresentationPrefs>) };
    } catch {
      return { ...DEFAULTS };
    }
  }

  export const presentation = $state<PresentationPrefs>(load());

  function persist(): void {
    try {
      localStorage.setItem(KEY, JSON.stringify(presentation));
    } catch {
      // Storage full/blocked: the prefs stay in-memory for the session.
    }
  }

  export function setPresentation(partial: Partial<PresentationPrefs>): void {
    Object.assign(presentation, partial);
    persist();
  }
</script>
