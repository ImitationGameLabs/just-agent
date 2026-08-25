<script lang="ts">
  // One row = one device: the enrolled tagma's identity (presence, agents)
  // joined with its host-side instance process by the slug prefix convention
  // (`tagma-<id8>`). Rows cover the three shapes the join produces: merged,
  // identity-only (enrolled, never spawned here), and process-only (an
  // instance whose slug carries no enrolled identity). Presentational only:
  // every action is delegated through props, mirroring the card components.
  import {
    formatTagmaStatusLine,
    presenceDotClass,
    type TagmaPresence,
    type TagmaStatusSummary,
  } from "../../lib/tagmata.svelte.ts";
  import {
    manage_instances_running,
    manage_instances_stop,
    manage_instances_stopped,
    nav_chat,
    tagma_presence_offline,
    tagma_presence_online,
    tagma_profile_unnamed,
  } from "../../paraglide/messages.js";

  let {
    identity = undefined,
    process = undefined,
    onStop,
  }: {
    // Present when an enrolled tagma backs this row.
    identity?: {
      tagmaId: string;
      label: string | null;
      presence: TagmaPresence;
      status?: TagmaStatusSummary;
    };
    // Present when a host-side instance backs this row.
    process?: {
      slug: string;
      workspace: string;
      running: boolean;
      port?: number;
    };
    onStop?: (slug: string) => void;
  } = $props();

  const name = $derived(
    identity ? (identity.label ?? tagma_profile_unnamed()) : process?.slug,
  );

  // Identity rows lead with the live status line (agents · tokens) and fall
  // back to the presence word while no snapshot has arrived; process-only
  // rows show the workspace. Merged rows append the workspace path.
  const subline = $derived.by(() => {
    const parts: string[] = [];
    if (identity) {
      if (identity.status) parts.push(formatTagmaStatusLine(identity.status));
      else if (identity.presence === "online") {
        parts.push(tagma_presence_online());
      } else if (identity.presence === "offline") {
        parts.push(tagma_presence_offline());
      }
    }
    if (process?.workspace) parts.push(process.workspace);
    return parts.join(" · ");
  });

  // Blank while presence is unresolved ("checking"): an identity-only row
  // must not read as offline before realtime's snapshot arrives.
  const stateText = $derived(
    process
      ? process.running
        ? manage_instances_running()
        : manage_instances_stopped()
      : identity?.presence === "online"
        ? tagma_presence_online()
        : identity?.presence === "offline"
          ? tagma_presence_offline()
          : "",
  );
</script>

<div class="p-4 flex items-center gap-4">
  {#if identity}
    <span
      class="size-2 rounded-full shrink-0 {presenceDotClass(identity.presence)}"
      aria-hidden="true"
    ></span>
  {:else}
    <!-- A process-only row has no liveness signal on the identity side; the
         neutral surface dot keeps the column aligned. -->
    <span
      class="size-2 rounded-full shrink-0 bg-surface-300-700"
      aria-hidden="true"
    ></span>
  {/if}
  <div class="min-w-0 flex-1">
    <p class="font-medium truncate text-sm">{name}</p>
    {#if subline}
      <p class="text-xs opacity-70 truncate">{subline}</p>
    {/if}
  </div>
  <div class="flex items-center gap-3 shrink-0">
    {#if process?.port}
      <a
        class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-surface-500"
        href={"/connect?tagmaUrl=http://127.0.0.1:" + process.port}
        >{nav_chat()}</a
      >
    {/if}
    {#if process?.running && onStop}
      <button
        type="button"
        class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-error-500"
        onclick={() => process && onStop(process.slug)}
      >
        {manage_instances_stop()}
      </button>
    {/if}
    <span
      class="text-sm {process?.running
        ? 'text-success-500 dark:text-success-400'
        : 'opacity-70'}"
    >
      {stateText}
    </span>
  </div>
</div>
