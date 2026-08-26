<script lang="ts">
  // Profiles manage page — card-based layered view (provider cards in a
  // global pool, tier containers holding profile cards, a parking area for
  // profiles out of rotation), matching the wire shape 1:1. Read-mostly:
  // editing goes through the Provider/Tier/Parking dialogs; profile cards
  // drag between tiers and parking (HTML5 DnD updating the draft).
  //
  // Probe results route inline to the card that triggered them: the page
  // accumulates providerReports/profileReports maps keyed by id, because the
  // store's single `probe` field is replaced wholesale on every call.
  import { profilesStore } from "../../lib/manage/profiles.svelte.ts";
  import { managementBackend } from "../../lib/manage/client.ts";
  import { refreshParkedLive as fetchParkedLive } from "../../lib/manage/parkedLive.ts";
  import { SvelteMap } from "svelte/reactivity";
  import ProfilesToolbar from "../../components/manage/ProfilesToolbar.svelte";
  import ProvidersSection from "../../components/manage/ProvidersSection.svelte";
  import TiersSection from "../../components/manage/TiersSection.svelte";
  import ParkingSection from "../../components/manage/ParkingSection.svelte";
  import ConfirmDialog from "../../components/ConfirmDialog.svelte";
  import ProviderDialog from "../../components/manage/ProviderDialog.svelte";
  import TierDialog from "../../components/manage/TierDialog.svelte";
  import ParkingDialog from "../../components/manage/ParkingDialog.svelte";
  import {
    moveProfile,
    moveFromParking,
    moveToParking,
    replaceTierProfiles,
    replaceParkingProfiles,
    singleProfileProbeRequest,
    singleParkingProfileProbeRequest,
    upsertProvider,
  } from "../../lib/manage/compute.ts";
  import {
    clearProfileResult,
    mergeProfileScope,
    mergeProfileScopeAll,
    mergeProviderScope,
    occupiedIdsOf,
    providerIdsOf,
  } from "../../lib/manage/profiles-view.ts";
  import type {
    ProfileProvider,
    ProfileProviderProbeReport,
    ProfileModelProbeReport,
  } from "@kallipai/kallip-client";
  import {
    common_remove,
    manage_profiles_apply,
    manage_profiles_apply_desc,
    manage_profiles_apply_desc_parked,
    manage_profiles_apply_title,
    manage_profiles_applied_result,
    manage_profiles_remove_tier_confirm_desc,
    manage_profiles_remove_tier_confirm_title,
    manage_profiles_title,
  } from "../../paraglide/messages.js";

  let { basePath = "/local/manage" }: { basePath?: string } = $props();
  $effect(() => {
    profilesStore.refresh();
  });

  let showApplyDialog = $state(false);
  let applyResult = $state<string | null>(null);

  async function onApply() {
    applyResult = null;
    try {
      const r = await profilesStore.apply();
      showApplyDialog = false;
      applyResult = manage_profiles_applied_result({
        applied: r.applied,
        skipped: r.skipped,
      });
    } catch {
      // Error surfaced via store
    }
  }

  async function onSave() {
    await profilesStore.save().catch(() => {});
    providerReports.clear();
    profileReports.clear();
  }

  // --- inline probe results, routed by call-site scope ---

  const providerReports = new SvelteMap<string, ProfileProviderProbeReport>();
  const profileReports = new SvelteMap<string, ProfileModelProbeReport>();

  async function onTestProvider(id: string) {
    await profilesStore.probeProvider(id);
    if (profilesStore.probe) {
      mergeProviderScope(providerReports, profilesStore.probe);
    }
  }

  async function onTestTier(tierIdx: number) {
    await profilesStore.probeTier(tierIdx);
    if (profilesStore.probe) {
      mergeProfileScope(tierIdx, profileReports, profilesStore.probe);
    }
  }

  async function onTestProfile(tierIdx: number, profileIdx: number) {
    const draft = profilesStore.draft;
    if (!draft) return;
    const body = singleProfileProbeRequest(
      profilesStore.config,
      draft,
      tierIdx,
      profileIdx,
    );
    if (!body) return;
    const resp = await profilesStore.probeRaw(body);
    if (!resp) return;
    mergeProviderScope(providerReports, resp);
    mergeProfileScope(tierIdx, profileReports, resp);
  }

  async function onTestAll() {
    await profilesStore.probeAll();
    if (!profilesStore.probe) return;
    mergeProviderScope(providerReports, profilesStore.probe);
    mergeProfileScopeAll(profileReports, profilesStore.probe);
  }

  function onDiscard() {
    profilesStore.reset();
    providerReports.clear();
    profileReports.clear();
    parkedLive = null;
  }

  // --- drag & drop (profile cards between tiers and the parking area) ---

  interface DragPayload {
    area: "tier" | "parking";
    fromTier: number;
    fromIdx: number;
  }

  let drag = $state<DragPayload | null>(null);
  let dragOverTier = $state(-1);
  let dragOverParking = $state(false);

  // Shared drag-end reset for both drop targets (card sections own the
  // markup, the page owns the drag state).
  function clearDrag(): void {
    drag = null;
    dragOverTier = -1;
    dragOverParking = false;
  }

  function onDropTier(toTier: number): void {
    const d = drag;
    drag = null;
    dragOverTier = -1;
    dragOverParking = false;
    const draft = profilesStore.draft;
    if (!d || !draft) return;
    if (d.area === "parking") {
      // parking → tier: the p:-keyed report is area-scoped, clear it.
      const id = draft.parking?.[d.fromIdx]?.id;
      if (id) profileReports.delete(`p:${id}`);
      profilesStore.draft = moveFromParking(draft, d.fromIdx, toTier);
      void refreshParkedLive();
      return;
    }
    if (d.fromTier !== toTier) {
      // Cross-tier: the key is tier-scoped, so clear the stale source entry
      // (same-tier keeps its key — the report survives the reorder).
      const id = draft.tiers[d.fromTier]?.profiles[d.fromIdx]?.id;
      if (id) clearProfileResult(profileReports, d.fromTier, id);
    }
    profilesStore.draft = moveProfile(draft, d.fromTier, d.fromIdx, toTier);
  }

  function onDropParking(): void {
    const d = drag;
    drag = null;
    dragOverTier = -1;
    dragOverParking = false;
    const draft = profilesStore.draft;
    if (!d || !draft || d.area !== "tier") return;
    // tier → parking: clear the tier-scoped source entry; the card will
    // re-key its report as p:<id> on the next parking Test.
    const id = draft.tiers[d.fromTier]?.profiles[d.fromIdx]?.id;
    if (id) clearProfileResult(profileReports, d.fromTier, id);
    profilesStore.draft = moveToParking(draft, d.fromTier, d.fromIdx);
    void refreshParkedLive();
  }

  // --- parked-live warn snapshot (event-driven, advisory) ---

  /** Parked ids some live agent still runs, from the last snapshot.
   * Null = no snapshot yet (or nothing parked-live); the banner and the
   * apply-confirm extension both render from it. Refreshed by parking/
   * unparking mutations, cleared on discard — never polled (a later
   * always-on variant needs the list endpoint to carry the active profile
   * id; that is a backend change, not a frontend poll). */
  let parkedLive = $state<{ agentCount: number; profileIds: string[] } | null>(
    null,
  );

  // parkedLive.ts owns the roster fetch and the allSettled fan-out; the
  // wrapper keeps the previous snapshot when the roster itself fails
  // (advisory only).
  async function refreshParkedLive(): Promise<void> {
    const res = await fetchParkedLive(
      managementBackend(),
      profilesStore.draft?.parking ?? [],
    );
    if (res.refreshed) parkedLive = res.snapshot;
  }

  // --- dialogs ---

  let providerDialog = $state<{
    open: boolean;
    mode: "new" | "edit";
    provider: ProfileProvider | null;
  }>({ open: false, mode: "new", provider: null });

  function openProviderNew() {
    providerDialog = { open: true, mode: "new", provider: null };
  }

  function openProviderEdit(ep: ProfileProvider) {
    providerDialog = { open: true, mode: "edit", provider: ep };
  }

  function onProviderSave(result: {
    id: string;
    family: string;
    baseUrl: string | null;
    apiKey: string | null;
  }) {
    const draft = profilesStore.draft;
    if (!draft) return;
    const existing =
      result.apiKey === null
        ? (draft.endpoints[result.id]?.api_key ?? "")
        : result.apiKey;
    profilesStore.draft = upsertProvider(draft, {
      id: result.id,
      family: result.family,
      api_key: existing,
      base_url: result.baseUrl,
    });
    providerDialog.open = false;
  }

  function onProviderRemove() {
    if (providerDialog.mode === "edit" && providerDialog.provider) {
      profilesStore.removeProvider(providerDialog.provider.id);
      providerReports.delete(providerDialog.provider.id);
    }
    providerDialog.open = false;
  }

  // Tier removal confirm: every removal rebinds agents (positional tiers),
  // so the kebab Remove always opens a confirm before mutating the draft.
  let removeTierIdx = $state<number | null>(null);

  function onTierRemoveConfirmed() {
    if (removeTierIdx === null) return;
    profilesStore.removeTier(removeTierIdx);
    profileReports.clear();
    removeTierIdx = null;
  }
  let tierDialog = $state<{ open: boolean; tierIdx: number }>({
    open: false,
    tierIdx: 0,
  });

  function onTierSave(
    rows: {
      id: string;
      endpoint: string;
      model: string;
      max_context_window: number;
    }[],
  ) {
    const draft = profilesStore.draft;
    if (!draft) return;
    profilesStore.draft = replaceTierProfiles(draft, tierDialog.tierIdx, rows);
    tierDialog.open = false;
  }

  // Parking dialog: single-profile form (see ParkingDialog). idx indexes the
  // draft's parked list in edit mode.
  let parkingDialog = $state<{
    open: boolean;
    mode: "new" | "edit";
    idx: number;
  }>({ open: false, mode: "new", idx: 0 });
  // Latest in-form probe result, rendered inside the dialog.
  let parkingProbeReport = $state<{
    status: string;
    detail: string | null;
  } | null>(null);

  function openParkingNew() {
    parkingProbeReport = null;
    parkingDialog = { open: true, mode: "new", idx: 0 };
  }

  function openParkingEdit(idx: number) {
    parkingProbeReport = null;
    parkingDialog = { open: true, mode: "edit", idx };
  }

  function onParkingSave(values: {
    id: string;
    endpoint: string;
    model: string;
    max_context_window: number;
  }) {
    const draft = profilesStore.draft;
    if (!draft) return;
    const list = [...(draft.parking ?? [])];
    if (parkingDialog.mode === "new") list.push(values);
    else list[parkingDialog.idx] = values;
    profilesStore.draft = replaceParkingProfiles(draft, list);
    parkingDialog.open = false;
    void refreshParkedLive();
  }

  function onParkingRemove() {
    const draft = profilesStore.draft;
    if (draft && parkingDialog.mode === "edit") {
      const id = draft.parking?.[parkingDialog.idx]?.id;
      profilesStore.draft = replaceParkingProfiles(
        draft,
        (draft.parking ?? []).filter((_, i) => i !== parkingDialog.idx),
      );
      if (id) profileReports.delete(`p:${id}`);
    }
    void refreshParkedLive();
    parkingDialog.open = false;
  }

  /** Probe the dialog's current form values without touching the draft:
   * stage them into a throwaway copy and reuse the parked-profile request
   * builder (committed config passed for the masked-key rule). */
  async function onParkingTest(values: {
    id: string;
    endpoint: string;
    model: string;
    max_context_window: number;
  }) {
    const draft = profilesStore.draft;
    if (!draft) return;
    const staged = replaceParkingProfiles(draft, [
      ...(draft.parking ?? []),
      values,
    ]);
    const body = singleParkingProfileProbeRequest(
      profilesStore.config,
      staged,
      (staged.parking?.length ?? 1) - 1,
    );
    if (!body) return;
    const resp = await profilesStore.probeRaw(body);
    if (!resp) return;
    mergeProviderScope(providerReports, resp);
    const p = resp.tiers[0]?.profiles[0];
    if (p) parkingProbeReport = { status: p.status, detail: p.detail ?? null };
  }

  /** Kebab Test on a parked card: same request shape, from the draft. */
  async function onTestParking(idx: number) {
    const draft = profilesStore.draft;
    if (!draft) return;
    const body = singleParkingProfileProbeRequest(
      profilesStore.config,
      draft,
      idx,
    );
    if (!body) return;
    const resp = await profilesStore.probeRaw(body);
    if (!resp) return;
    mergeProviderScope(providerReports, resp);
    const p = resp.tiers[0]?.profiles[0];
    if (p) {
      const id = draft.parking?.[idx]?.id ?? p.profile_id;
      profileReports.set(`p:${id}`, p);
    }
  }

  const providerIds = $derived(providerIdsOf(profilesStore.draft));

  const occupiedIds = $derived(occupiedIdsOf(profilesStore.draft));
</script>

<svelte:head><title>{manage_profiles_title()}</title></svelte:head>

<div class="h-full overflow-y-auto">
  <div class="p-6 max-w-3xl space-y-6">
    <ProfilesToolbar
      store={profilesStore}
      {applyResult}
      {parkedLive}
      {onTestAll}
      {onSave}
      {onDiscard}
      onRequestApply={() => (showApplyDialog = true)}
    />

    <!-- Providers: global pool of provider cards -->
    {#if profilesStore.draft}
      <ProvidersSection
        providers={Object.values(profilesStore.draft.endpoints)}
        reports={providerReports}
        isProbing={profilesStore.isProbing}
        onTest={onTestProvider}
        onEdit={openProviderEdit}
        onAdd={openProviderNew}
      />

      <TiersSection
        tiers={profilesStore.draft.tiers}
        reports={profileReports}
        isProbing={profilesStore.isProbing}
        {dragOverTier}
        onCardDragStart={(fromTier, fromIdx) =>
          (drag = { area: "tier", fromTier, fromIdx })}
        onCardDragEnd={clearDrag}
        onTierDragOver={(tierIdx) => (dragOverTier = tierIdx)}
        onTierDragLeave={(tierIdx) =>
          (dragOverTier = tierIdx === dragOverTier ? -1 : dragOverTier)}
        onTierDrop={onDropTier}
        {onTestTier}
        {onTestProfile}
        onEditTier={(tierIdx) => (tierDialog = { open: true, tierIdx })}
        onRemoveTier={(tierIdx) => (removeTierIdx = tierIdx)}
        onAddTier={() => profilesStore.addTier()}
      />

      <ParkingSection
        parking={profilesStore.draft.parking ?? []}
        reports={profileReports}
        isProbing={profilesStore.isProbing}
        {dragOverParking}
        onCardDragStart={(fromIdx) =>
          (drag = { area: "parking", fromTier: -1, fromIdx })}
        onCardDragEnd={clearDrag}
        onParkingDragOver={() => (dragOverParking = true)}
        onParkingDragLeave={() => (dragOverParking = false)}
        onParkingDrop={onDropParking}
        onTest={onTestParking}
        onEdit={openParkingEdit}
        onAdd={openParkingNew}
      />
    {/if}
  </div>
</div>

<ConfirmDialog
  busy={profilesStore.isSaving}
  open={showApplyDialog}
  title={manage_profiles_apply_title()}
  description={parkedLive
    ? `${manage_profiles_apply_desc()} ${manage_profiles_apply_desc_parked({
        count: parkedLive.agentCount,
      })}`
    : manage_profiles_apply_desc()}
  confirmLabel={manage_profiles_apply()}
  tone="primary"
  onConfirm={onApply}
  onCancel={() => (showApplyDialog = false)}
/>

<ConfirmDialog
  open={removeTierIdx !== null}
  title={manage_profiles_remove_tier_confirm_title()}
  description={manage_profiles_remove_tier_confirm_desc()}
  confirmLabel={common_remove()}
  tone="danger"
  onConfirm={onTierRemoveConfirmed}
  onCancel={() => (removeTierIdx = null)}
/>

<ProviderDialog
  open={providerDialog.open}
  mode={providerDialog.mode}
  provider={providerDialog.provider}
  existingIds={providerIds}
  onSave={onProviderSave}
  onCancel={() => (providerDialog.open = false)}
  onRemove={providerDialog.mode === "edit" ? onProviderRemove : null}
/>

<TierDialog
  open={tierDialog.open}
  tierIdx={tierDialog.tierIdx}
  profiles={profilesStore.draft?.tiers[tierDialog.tierIdx]?.profiles ?? []}
  {providerIds}
  onSave={onTierSave}
  onCancel={() => (tierDialog.open = false)}
/>

<ParkingDialog
  open={parkingDialog.open}
  mode={parkingDialog.mode}
  profile={profilesStore.draft?.parking?.[parkingDialog.idx] ?? null}
  {providerIds}
  {occupiedIds}
  probeReport={parkingProbeReport}
  onSave={onParkingSave}
  onCancel={() => (parkingDialog.open = false)}
  onTest={onParkingTest}
  onRemove={parkingDialog.mode === "edit" ? onParkingRemove : null}
/>
