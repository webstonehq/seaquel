<script lang="ts">
	import CanvasSidebar from "./canvas-sidebar.svelte";
	import CanvasWorkspace from "./canvas-workspace.svelte";
</script>

<!-- Use flex-1 to fill the parent flex container -->
<div class="flex flex-1 min-h-0 w-full overflow-hidden">
	<!-- Canvas-specific sidebar for adding nodes, saved canvases, etc. -->
	<CanvasSidebar />

	<!-- Canvas workspace - needs flex container for child to use flex-1 -->
	<div class="flex flex-col flex-1 min-h-0 min-w-0">
		<CanvasWorkspace />
	</div>
</div>
