<script lang="ts">
	import { page } from '$app/state';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import { Button } from '$lib/components/ui/button/index.js';
	import VideoStatsPanel from '$lib/components/video-stats-panel.svelte';
	let bvid = $derived(page.params.bvid ?? '');
	let sourceId = $derived(Number(page.url.searchParams.get('source')));
	let hasSource = $derived(Number.isSafeInteger(sourceId) && sourceId > 0);
	let returnHref = $derived(hasSource ? `/dynamic-sources/${sourceId}` : '/videos');
	$effect(() => {
		setBreadcrumb(
			hasSource
				? [
						{ label: '动态源', href: '/dynamic-sources' },
						{ label: '动态源详情', href: returnHref },
						{ label: '视频数据' }
					]
				: [{ label: '视频', href: '/videos' }, { label: '视频数据' }, { label: bvid }]
		);
	});
</script>

<svelte:head><title>{bvid} 视频数据 - Bili Eye</title></svelte:head>
<Button size="sm" variant="outline" href={returnHref}
	>{hasSource ? '返回动态源详情' : '返回视频列表'}</Button
>
<VideoStatsPanel {bvid} showTitle />
