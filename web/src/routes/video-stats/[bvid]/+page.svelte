<script lang="ts">
	import { page } from '$app/state';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import VideoStatsPanel from '$lib/components/video-stats-panel.svelte';
	let bvid = $derived(page.params.bvid ?? '');
	$effect(() => {
		setBreadcrumb([{ label: '视频数据' }, { label: bvid }]);
	});
</script>

<svelte:head><title>{bvid} 视频数据 - Bili Sync</title></svelte:head>
<VideoStatsPanel {bvid} showTitle />
