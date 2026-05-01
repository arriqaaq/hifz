<script lang="ts">
  import {
    type EntityKind,
    KIND_COLOR,
    KIND_ICON,
    shortId,
    entityHref,
  } from './entityHelpers';

  type Props = {
    kind: EntityKind;
    id: string;
    label?: string;
    size?: 'sm' | 'md';
    href?: string | null;
    onclick?: () => void;
    title?: string;
  };

  let {
    kind,
    id,
    label,
    size = 'md',
    href,
    onclick,
    title,
  }: Props = $props();

  let resolvedHref = $derived(href === null ? null : (href ?? entityHref(kind, id)));
  let display = $derived(label ?? shortId(id));
  let Icon = $derived(KIND_ICON[kind]);
  let color = $derived(KIND_COLOR[kind]);
  let tooltip = $derived(title ?? `${kind}: ${id}`);
</script>

{#snippet inner()}
  <span class="icon"><Icon size={size === 'sm' ? 11 : 12} strokeWidth={1.8} /></span>
  <span class="label">{display}</span>
{/snippet}

{#if resolvedHref}
  <a
    class={`chip chip-${kind} chip-${size}`}
    href={resolvedHref}
    style={`--chip: ${color}`}
    title={tooltip}
    onclick={onclick}
  >
    {@render inner()}
  </a>
{:else if onclick}
  <button
    type="button"
    class={`chip chip-${kind} chip-${size}`}
    style={`--chip: ${color}`}
    title={tooltip}
    onclick={onclick}
  >
    {@render inner()}
  </button>
{:else}
  <span class={`chip chip-${kind} chip-${size}`} style={`--chip: ${color}`} title={tooltip}>
    {@render inner()}
  </span>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 8px 1px 6px;
    color: var(--chip);
    background: color-mix(in srgb, var(--chip) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--chip) 28%, transparent);
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 11px;
    font-weight: 500;
    line-height: 1.4;
    text-decoration: none;
    transition: background 120ms, border-color 120ms;
    max-width: 220px;
  }

  a.chip:hover,
  button.chip:hover {
    background: color-mix(in srgb, var(--chip) 14%, transparent);
    border-color: color-mix(in srgb, var(--chip) 45%, transparent);
  }

  button.chip {
    cursor: pointer;
  }

  .chip-sm {
    font-size: 10px;
    padding: 0 6px 0 5px;
  }

  /* shape variations */
  .chip-session {
    border-radius: 4px;
  }
  .chip-run {
    border-radius: 2px;
    transform: skewX(-8deg);
  }
  .chip-run :global(.icon),
  .chip-run :global(.label) {
    transform: skewX(8deg);
  }
  .chip-observation {
    border-radius: 999px;
  }
  .chip-memory {
    border-radius: 4px;
    clip-path: polygon(8% 0, 92% 0, 100% 50%, 92% 100%, 8% 100%, 0 50%);
    padding: 1px 14px 1px 12px;
  }
  .chip-commit {
    border-radius: 4px;
    clip-path: polygon(8px 0, 100% 0, 100% 100%, 8px 100%, 0 50%);
    padding-left: 10px;
  }
  .chip-project {
    border-radius: 999px;
  }

  .icon {
    display: inline-flex;
    align-items: center;
    color: var(--chip);
    flex-shrink: 0;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
