<script lang="ts">
  import TagmataSection from "../components/tagmata/TagmataSection.svelte";
  import { tagmata_title } from "../paraglide/messages.js";
</script>

<svelte:head><title>{tagmata_title()}</title></svelte:head>

<!-- Thin /tagmata route: the registry lives in TagmataSection so this page
     and the unified /instances page share one source. -->
<TagmataSection />
