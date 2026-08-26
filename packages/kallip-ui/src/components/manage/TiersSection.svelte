<script lang="ts">
  // Tiers pool for the profiles page: one draggable-profile container per
  // tier, the tier probe footer, and the dashed add-tier card. Drag state
  // and drop mutations stay in the page; this section only reports the raw
  // drag lifecycle (start/over/leave/end/drop) so both drop targets share
  // one source of truth.
  import type { SvelteMap } from "svelte/reactivity";
  import { Menu, Portal } from "@skeletonlabs/skeleton-svelte";
  import {
    FlaskConical,
    MoreVertical,
    Pencil,
    Plus,
    Trash,
  } from "@lucide/svelte";
  import type {
    ProfileModel,
    ProfileModelProbeReport,
    ProfileTier,
  } from "@kallipai/kallip-client";
  import { TONAL_ICON_SURF } from "../../lib/classes.ts";
  import ProfileCard from "./ProfileCard.svelte";
  import {
    profileKey,
    probeStatusColor,
    probeStatusLabel,
  } from "../../lib/manage/profiles-view.ts";
  import {
    common_edit,
    common_remove,
    manage_profiles_add_tier,
    manage_profiles_max_context_label,
    manage_profiles_profile_actions_aria,
    manage_profiles_profile_model_label,
    manage_profiles_profile_provider_label,
    manage_profiles_probe_tier_fail,
    manage_profiles_probe_tier_ok,
    manage_profiles_test,
    manage_profiles_test_all,
    manage_profiles_tier,
    manage_profiles_tier_actions_aria,
    manage_profiles_tier_drop_here,
    manage_profiles_tiers,
    manage_profiles_tiers_desc_l1,
    manage_profiles_tiers_desc_l2,
    manage_profiles_tiers_desc_l3,
    manage_profiles_tiers_desc_l4,
  } from "../../paraglide/messages.js";

  let {
    tiers,
    reports,
    isProbing,
    dragOverTier,
    onCardDragStart,
    onCardDragEnd,
    onTierDragOver,
    onTierDragLeave,
    onTierDrop,
    onTestTier,
    onTestProfile,
    onEditTier,
    onRemoveTier,
    onAddTier,
  }: {
    tiers: readonly ProfileTier[];
    reports: SvelteMap<string, ProfileModelProbeReport>;
    isProbing: boolean;
    dragOverTier: number;
    onCardDragStart: (
      fromTier: number,
      fromIdx: number,
      id: string,
      e: DragEvent,
    ) => void;
    onCardDragEnd: () => void;
    onTierDragOver: (tierIdx: number) => void;
    onTierDragLeave: (tierIdx: number) => void;
    onTierDrop: (tierIdx: number) => void;
    onTestTier: (tierIdx: number) => void;
    onTestProfile: (tierIdx: number, profileIdx: number) => void;
    onEditTier: (tierIdx: number) => void;
    onRemoveTier: (tierIdx: number) => void;
    onAddTier: () => void;
  } = $props();
</script>

<section class="space-y-3">
  <h2 class="text-sm font-medium uppercase opacity-60 tracking-wide">
    {manage_profiles_tiers()}
  </h2>
  <div class="text-xs opacity-60 mt-1 space-y-0.5">
    <p>{manage_profiles_tiers_desc_l1()}</p>
    <p>{manage_profiles_tiers_desc_l2()}</p>
    <p>{manage_profiles_tiers_desc_l3()}</p>
    <p>{manage_profiles_tiers_desc_l4()}</p>
  </div>

  {#each tiers as tier, tierIdx (tierIdx)}
    {@const tierReport = [...reports.entries()]
      .filter(([k]) => k.startsWith(`${tierIdx}:`))
      .map(([, v]) => v)}
    <div
      role="list"
      class="card preset-tonal-surface p-4 space-y-3 {dragOverTier === tierIdx
        ? 'outline-2 outline-dashed outline-primary-500'
        : ''}"
      ondragover={(e) => {
        e.preventDefault();
        onTierDragOver(tierIdx);
      }}
      ondragleave={() => onTierDragLeave(tierIdx)}
      ondrop={(e) => {
        e.preventDefault();
        onTierDrop(tierIdx);
      }}
    >
      <div class="flex items-center justify-between gap-2">
        <div class="text-sm font-medium">
          {manage_profiles_tier()}
          <span class="font-mono opacity-80">#{tierIdx}</span>
        </div>
        <Menu
          positioning={{ placement: "bottom-end" }}
          onSelect={(e) => {
            if (e.value === "test") onTestTier(tierIdx);
            else if (e.value === "edit") onEditTier(tierIdx);
            else if (e.value === "remove") onRemoveTier(tierIdx);
          }}
        >
          <Menu.Trigger
            class="size-10 {TONAL_ICON_SURF} shrink-0"
            aria-label={manage_profiles_tier_actions_aria()}
            disabled={isProbing}
          >
            <MoreVertical class="size-4" />
          </Menu.Trigger>
          <Portal>
            <Menu.Positioner>
              <Menu.Content class="card preset-tonal-surface p-1 min-w-[8rem]">
                <Menu.Item
                  value="test"
                  class="flex items-center gap-2 px-3 py-2 rounded-base text-sm cursor-pointer hover:preset-filled-surface-500"
                >
                  <FlaskConical class="size-4" />
                  {manage_profiles_test_all()}
                </Menu.Item>
                <Menu.Item
                  value="edit"
                  class="flex items-center gap-2 px-3 py-2 rounded-base text-sm cursor-pointer hover:preset-filled-surface-500"
                >
                  <Pencil class="size-4" />
                  {common_edit()}
                </Menu.Item>
                <Menu.Item
                  value="remove"
                  class="flex items-center gap-2 px-3 py-2 rounded-base text-sm text-error-500 dark:text-error-400 cursor-pointer hover:preset-filled-error-500"
                >
                  <Trash class="size-4" />
                  {common_remove()}
                </Menu.Item>
              </Menu.Content>
            </Menu.Positioner>
          </Portal>
        </Menu>
      </div>

      {#each tier.profiles as profile, profileIdx (profileIdx)}
        {@const report = reports.get(profileKey(tierIdx, profile.id))}
        <ProfileCard
          {profile}
          {report}
          {isProbing}
          onDragStart={(e: DragEvent) =>
            onCardDragStart(tierIdx, profileIdx, profile.id, e)}
          onDragEnd={onCardDragEnd}
          onTest={() => onTestProfile(tierIdx, profileIdx)}
          onEdit={() => onEditTier(tierIdx)}
        />
      {/each}
      {#if tier.profiles.length === 0}
        <p class="text-xs opacity-50">
          {manage_profiles_tier_drop_here()}
        </p>
      {/if}

      <!-- Card footer: the tier probe summary (a result lands beside
           the kebab menu that produced it). -->
      {#if tierReport.length > 0}
        <div class="flex items-center gap-2 flex-wrap text-xs">
          {#if tierReport.every((r) => r.status === "ok")}
            <span class={probeStatusColor.ok}>
              {manage_profiles_probe_tier_ok()}
            </span>
          {:else}
            <span class={probeStatusColor.invalid_config}>
              {manage_profiles_probe_tier_fail()}
              {tierReport
                .filter((r) => r.status !== "ok")
                .map((r) => r.profile_id)
                .join(", ")}
            </span>
          {/if}
        </div>
      {/if}
    </div>
  {/each}

  <!-- Add-tier card: same level as the tier containers; appends an
       empty tier directly (no dialog) — drag profiles in or use a
       tier's Edit. An empty tier cannot be saved (PUT rejects), by
       design: fill it before saving. -->
  <button
    type="button"
    class="card preset-tonal-surface border-2 border-dashed border-surface-400 p-4 flex items-center justify-center gap-2 min-h-24 w-full hover:preset-filled-surface-100-900 transition cursor-pointer"
    onclick={onAddTier}
  >
    <Plus class="size-6 opacity-70" />
    <span class="text-sm opacity-70">
      {manage_profiles_add_tier()}
    </span>
  </button>
</section>
