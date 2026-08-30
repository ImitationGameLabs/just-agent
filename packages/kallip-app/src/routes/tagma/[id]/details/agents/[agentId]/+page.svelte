<script lang="ts">
  import { OnlineAgentDetailPage } from "@kallipai/kallip-ui";
  import { page } from "$app/state";
</script>

<OnlineAgentDetailPage
  tagmaId={page.params.id ?? ""}
  agentId={page.params.agentId ?? ""}
/>
