<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import api from '$lib/api';
	import type { ApiError, VideoPlaybackResponse } from '$lib/types';

	let {
		open = $bindable(false),
		videoId,
		title
	}: { open?: boolean; videoId: number; title: string } = $props();
	let pages = $state<VideoPlaybackResponse['pages']>([]);
	let selectedId = $state<number | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let playerError = $state<string | null>(null);
	let sequence = 0;
	const selectedPage = $derived(pages.find((page) => page.id === selectedId));

	async function load(id: number) {
		const request = ++sequence;
		loading = true;
		pages = [];
		selectedId = null;
		error = null;
		playerError = null;
		try {
			const response = await api.createVideoPlayback(id);
			if (request !== sequence) return;
			pages = response.data.pages;
			selectedId = pages[0]?.id ?? null;
		} catch (cause) {
			if (request === sequence) error = (cause as ApiError).message;
		} finally {
			if (request === sequence) loading = false;
		}
	}

	$effect(() => {
		if (open) load(videoId);
		return () => {
			sequence += 1;
		};
	});

	function selectPage(id: number) {
		selectedId = id;
		playerError = null;
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-h-[90dvh] overflow-y-auto sm:max-w-5xl">
		<Dialog.Header>
			<Dialog.Title class="pr-6 break-words">{title}</Dialog.Title>
			<Dialog.Description>播放服务器上已下载的视频，B 站失效不影响本地副本。</Dialog.Description>
		</Dialog.Header>
		{#if open}
			{#if loading}
				<p role="status" class="text-muted-foreground py-12 text-center">正在查找本地视频...</p>
			{:else if error}
				<div class="space-y-3 py-6 text-center">
					<p role="alert">{error}</p>
					<Button variant="outline" onclick={() => load(videoId)}>重新加载</Button>
				</div>
			{:else if selectedPage}
				{#key selectedPage.url}
					<!-- svelte-ignore a11y_media_has_caption (本地视频可能没有可用的字幕文件) -->
					<video
						class="max-h-[60dvh] w-full rounded-md bg-black"
						src={selectedPage.url}
						controls
						playsinline
						preload="metadata"
						onerror={() =>
							(playerError =
								'无法播放：浏览器可能不支持此视频编码，或文件已移除、播放链接已过期。可重新加载或下载后用本地播放器打开。')}
					></video>
				{/key}
				{#if playerError}
					<div class="space-y-2">
						<p role="alert" class="text-destructive text-sm">{playerError}</p>
						<Button size="sm" variant="outline" onclick={() => load(videoId)}>重新加载</Button>
					</div>
				{/if}
				<div class="flex flex-wrap items-center justify-between gap-2 text-sm">
					<p class="min-w-0 break-words">P{selectedPage.pid}：{selectedPage.name}</p>
					<a
						class="text-primary shrink-0 underline underline-offset-4"
						href={selectedPage.url}
						download={`${selectedPage.name}.mp4`}>下载此分 P</a
					>
				</div>
				{#if pages.length > 1}
					<div class="flex max-h-40 flex-wrap gap-2 overflow-y-auto" aria-label="选择分 P">
						{#each pages as page (page.id)}
							<Button
								class="max-w-full"
								title={`P${page.pid}：${page.name}`}
								size="sm"
								variant={selectedId === page.id ? 'default' : 'outline'}
								aria-pressed={selectedId === page.id}
								onclick={() => selectPage(page.id)}
								><span class="truncate">P{page.pid}：{page.name}</span></Button
							>
						{/each}
					</div>
				{/if}
			{/if}
		{/if}
	</Dialog.Content>
</Dialog.Root>
