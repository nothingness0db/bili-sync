<script lang="ts">
	import api from '$lib/api';
	import type { ApiError, VideoStatListResponse } from '$lib/types';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import Pagination from '$lib/components/pagination.svelte';
	let { sourceId, refreshKey = 0 }: { sourceId: number; refreshKey?: number } = $props();
	let data = $state<VideoStatListResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let days = $state<number | null>(7);
	let currentPage = $state(0);
	let sequence = 0;
	const ranges = [
		{ label: '7天', days: 7 },
		{ label: '30天', days: 30 },
		{ label: '90天', days: 90 },
		{ label: '全部', days: null }
	];
	const format = (value: number | null) => (value === null ? '—' : value.toLocaleString('zh-CN'));
	const growth = (value: number | null) =>
		value === null ? '—' : `${value > 0 ? '+' : ''}${format(value)}`;
	async function load(id: number, page: number, range: number | null) {
		const current = ++sequence;
		loading = true;
		error = null;
		try {
			const response = await api.getSourceVideoStats(id, page, range);
			if (current === sequence) data = response.data;
		} catch (e) {
			if (current === sequence) error = (e as ApiError).message;
		} finally {
			if (current === sequence) loading = false;
		}
	}
	$effect(() => {
		void refreshKey;
		void load(sourceId, currentPage, days);
	});
</script>

<section class="rounded-lg border p-4" aria-label="逐视频数据">
	<div class="mb-3 flex flex-wrap items-center justify-between gap-3">
		<div>
			<h2 class="text-sm font-medium">逐视频数据</h2>
			<p class="text-muted-foreground mt-1 text-xs">
				包含普通投稿和动态视频；增长按所选范围内的首末采样计算。
			</p>
		</div>
		<div class="flex flex-wrap gap-1">
			{#each ranges as range (range.label)}<Button
					variant={days === range.days ? 'default' : 'outline'}
					aria-pressed={days === range.days}
					size="sm"
					class="h-7 px-2 text-xs"
					onclick={() => {
						days = range.days;
						currentPage = 0;
					}}>{range.label}</Button
				>{/each}<Button
				variant="outline"
				size="sm"
				class="h-7 px-2 text-xs"
				disabled={loading}
				onclick={() => load(sourceId, currentPage, days)}>刷新</Button
			>
		</div>
	</div>
	{#if error}<p role="alert" class="text-destructive py-4 text-sm">加载失败：{error}</p>
	{:else if loading}<p class="text-muted-foreground py-6 text-center text-sm">正在加载视频数据…</p>
	{:else if data?.videos.length}
		<div class="overflow-x-auto">
			<Table.Root>
				<Table.Header
					><Table.Row
						><Table.Head>视频</Table.Head><Table.Head class="text-right">播放</Table.Head
						><Table.Head class="text-right">播放增长</Table.Head><Table.Head class="text-right"
							>点赞</Table.Head
						><Table.Head class="text-right">点赞增长</Table.Head><Table.Head>最近采样</Table.Head
						></Table.Row
					></Table.Header
				>
				<Table.Body
					>{#each data.videos as video (video.bvid)}<Table.Row>
							<Table.Cell class="min-w-[220px] max-w-[360px]"
								><a href={`/video-stats/${video.bvid}`} class="block font-medium hover:underline"
									>{video.title}</a
								><span class="text-muted-foreground text-xs">{video.bvid}</span></Table.Cell
							>
							<Table.Cell class="text-right tabular-nums"
								>{format(video.latest.viewCount)}</Table.Cell
							><Table.Cell
								class="text-right tabular-nums"
								title={video.baselineAt
									? `自 ${new Date(video.baselineAt).toLocaleString('zh-CN')} 起`
									: '尚无增长记录'}>{growth(video.growth.viewCount)}</Table.Cell
							>
							<Table.Cell class="text-right tabular-nums"
								>{format(video.latest.likeCount)}</Table.Cell
							><Table.Cell class="text-right tabular-nums"
								>{growth(video.growth.likeCount)}</Table.Cell
							>
							<Table.Cell class="whitespace-nowrap text-xs text-muted-foreground"
								>{new Date(video.latest.recordedAt).toLocaleString('zh-CN')}</Table.Cell
							>
						</Table.Row>{/each}</Table.Body
				>
			</Table.Root>
		</div>
		<p class="text-muted-foreground mt-2 text-xs">
			共 {data.totalCount} 个已采样视频 · 点击视频查看全部指标与趋势；“—”表示暂无足够采样。
		</p>
		<Pagination
			{currentPage}
			totalPages={Math.ceil(data.totalCount / 20)}
			onPageChange={(page) => (currentPage = page)}
		/>
	{:else}<p class="text-muted-foreground py-6 text-center text-sm">
			还没有逐视频快照，下一次账号同步会采集普通投稿和动态视频。
		</p>{/if}
</section>
