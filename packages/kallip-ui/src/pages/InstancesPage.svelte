<script lang="ts">
  import TagmataSection from "../components/tagmata/TagmataSection.svelte";
  import InstancesPanel from "./manage/InstancesPanel.svelte";
  import { manage_instances_title } from "../paraglide/messages.js";
</script>

<svelte:head><title>{manage_instances_title()}</title></svelte:head>

<!-- The unified instances page: the tagmata registry (enroll-code minting)
     on top, machine-level instance management (cards, spawn, stop) below.
     Mode-neutral by design -- the gate renders it in both families. -->
<div class="space-y-8">
  <section>
    <TagmataSection />
  </section>
  <section>
    <InstancesPanel />
  </section>
</div>
