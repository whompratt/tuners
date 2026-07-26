// Chart palette resolved from the CSS tokens once per draw, so the pure
// draw functions never touch the DOM.

export type Palette = {
  ink: string;
  ink2: string;
  muted: string;
  grid: string;
  baseline: string;
  accent: string;
  ok: string;
  danger: string;
  info: string;
};

export function palette(): Palette {
  const css = (n: string) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
  return {
    ink: css("--ink"),
    ink2: css("--ink-2"),
    muted: css("--muted"),
    grid: css("--grid"),
    baseline: css("--baseline"),
    accent: css("--accent"),
    ok: css("--ok"),
    danger: css("--danger"),
    info: css("--info"),
  };
}
