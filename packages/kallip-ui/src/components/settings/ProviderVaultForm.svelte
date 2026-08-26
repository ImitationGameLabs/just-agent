<script lang="ts">
  // The "add key" form. Prop-driven like AddPasskey: the owner passes the
  // submit callback (which throws on failure) and `canFlip`, which decides
  // the default storage mode -- encrypted when this session arrived via
  // passkey (the device vault key exists / will exist), plaintext otherwise
  // (an OAuth-only account has no flip affordance later, so encryption
  // would produce a row it could never offer to reveal inline).
  import type { ProviderKeyMode } from "@kallipai/kallip-agora-client";
  import { AgoraApiError } from "@kallipai/kallip-agora-client";
  import {
    settings_provider_new_title,
    settings_provider_name_label,
    settings_provider_name_placeholder,
    settings_provider_family_label,
    settings_provider_family_placeholder,
    settings_provider_base_url_label,
    settings_provider_key_label,
    settings_provider_badge_encrypted,
    settings_provider_badge_plaintext,
    settings_provider_mode_label,
    settings_provider_mode_encrypted_hint,
    settings_provider_mode_plaintext_hint,
    settings_provider_plain_only_hint,
    settings_provider_name_duplicate,
    settings_error_unknown,
    common_adding,
    common_cancel,
    common_create,
  } from "../../paraglide/messages.js";

  let {
    open = false,
    // Whether encrypted storage is offered; also picks the default mode.
    canFlip = false,
    onCreate,
    onClosed,
  }: {
    open?: boolean;
    canFlip?: boolean;
    // Commit the form fields (the store seals encrypted keys before send);
    // resolve true on success so the form resets, false/undefined lets the
    // user retry without retyping.
    onCreate?: (req: {
      name: string;
      provider: string;
      base_url: string | null;
      key_material: string;
      mode: ProviderKeyMode;
    }) => Promise<boolean> | boolean | void;
    onClosed?: () => void;
  } = $props();

  let name = $state("");
  let family = $state("");
  let baseUrl = $state("");
  let key = $state("");
  let mode = $state<ProviderKeyMode>("encrypted");
  let error = $state<string | null>(null);
  let busy = $state(false);

  function begin() {
    name = "";
    family = "";
    baseUrl = "";
    key = "";
    mode = canFlip ? "encrypted" : "plaintext";
    error = null;
  }

  begin();

  async function submit() {
    const trimmedName = name.trim();
    if (!trimmedName || !key || busy) return;
    busy = true;
    try {
      const ok =
        (await onCreate?.({
          name: trimmedName,
          provider: family.trim(),
          base_url: baseUrl.trim() || null,
          key_material: key,
          mode,
        })) ?? false;
      if (ok) {
        begin();
        onClosed?.();
      }
    } catch (e) {
      console.error("[vault] create failed:", e);
      error =
        e instanceof AgoraApiError && e.status === 409
          ? settings_provider_name_duplicate()
          : settings_error_unknown();
    } finally {
      busy = false;
    }
  }
</script>

{#if open}
  <div class="card preset-tonal-surface p-3 space-y-2">
    <div class="text-sm font-medium">{settings_provider_new_title()}</div>
    <label class="block space-y-1">
      <span class="text-xs opacity-60">{settings_provider_name_label()}</span>
      <input
        class="input input-sm w-full"
        placeholder={settings_provider_name_placeholder()}
        maxlength={64}
        bind:value={name}
        disabled={busy}
      />
    </label>
    <div class="flex flex-wrap gap-2">
      <label class="block flex-1 min-w-32 space-y-1">
        <span class="text-xs opacity-60"
          >{settings_provider_family_label()}</span
        >
        <input
          class="input input-sm w-full"
          placeholder={settings_provider_family_placeholder()}
          maxlength={64}
          bind:value={family}
          disabled={busy}
        />
      </label>
      <label class="block flex-1 min-w-32 space-y-1">
        <span class="text-xs opacity-60"
          >{settings_provider_base_url_label()}</span
        >
        <input
          class="input input-sm w-full font-mono"
          type="url"
          maxlength={256}
          bind:value={baseUrl}
          disabled={busy}
        />
      </label>
    </div>
    <label class="block space-y-1">
      <span class="text-xs opacity-60">{settings_provider_key_label()}</span>
      <input
        class="input input-sm w-full font-mono"
        type="password"
        autocomplete="off"
        bind:value={key}
        disabled={busy}
      />
    </label>
    <!-- Mode choice only when a choice exists; the plaintext-only hint takes
         its place otherwise, so an OAuth-only session sees why, not a dead
         radio. -->
    {#if canFlip}
      <fieldset class="space-y-1" disabled={busy}>
        <legend class="text-xs opacity-60">
          {settings_provider_mode_label()}
        </legend>
        <label class="flex items-center gap-2 text-xs select-none">
          <input type="radio" value="encrypted" bind:group={mode} />
          <span
            >{settings_provider_badge_encrypted()} —
            {settings_provider_mode_encrypted_hint()}</span
          >
        </label>
        <label class="flex items-center gap-2 text-xs select-none">
          <input type="radio" value="plaintext" bind:group={mode} />
          <span
            >{settings_provider_badge_plaintext()} —
            {settings_provider_mode_plaintext_hint()}</span
          >
        </label>
      </fieldset>
    {:else}
      <p class="text-xs opacity-60">{settings_provider_plain_only_hint()}</p>
    {/if}
    {#if error}
      <div class="text-xs text-error-600 dark:text-error-500">{error}</div>
    {/if}
    <div class="flex gap-2">
      <button
        class="btn btn-sm preset-filled-primary-500"
        disabled={!name.trim() || !key || busy}
        onclick={submit}
      >
        {busy ? common_adding() : common_create()}
      </button>
      <button
        class="btn btn-sm preset-tonal-surface"
        onclick={() => onClosed?.()}>{common_cancel()}</button
      >
    </div>
  </div>
{/if}
