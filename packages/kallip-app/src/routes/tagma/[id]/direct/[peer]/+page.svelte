<script lang="ts">
  import { page } from "$app/state";
  import { DirectSessionPage } from "@kallipai/kallip-ui";
</script>

<DirectSessionPage
  tagmaId={page.params.id ?? ""}
  peerId={page.params.peer ?? ""}
/>
