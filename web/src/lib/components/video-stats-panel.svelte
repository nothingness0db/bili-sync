<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Chart from '$lib/components/ui/chart/index.js';
	import MyChartTooltip from '$lib/components/custom/my-chart-tooltip.svelte';
	import { AreaChart } from 'layerchart';
	import { curveLinear } from 'd3-shape';
	import api from '$lib/api';
	import type { ApiError, VideoMetrics, VideoStatsResponse } from '$lib/types';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

	let { bvid, showTitle = false }: { bvid: string; showTitle?: boolean } = $props();
	let data = $state<VideoStatsResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let days = $state<number | null>(7);
	let selected = $state<keyof VideoMetrics>('viewCount');
	let sequence = 0;
	const ranges = [
		{ label: '7天', days: 7 },
		{ label: '30天', days: 30 },
		{ label: '90天', days: 90 },
		{ label: '全部', days: null }
	];
	const metrics: { key: keyof VideoMetrics; label: string; color: string }[] = [
		{ key: 'viewCount', label: '播放', color: 'var(--primary)' },
		{ key: 'likeCount', label: '点赞', color: '#ec4899' },
		{ key: 'coinCount', label: '投币', color: '#f59e0b' },
		{ key: 'favoriteCount', label: '收藏', color: '#22c55e' },
		{ key: 'shareCount', label: '分享', color: '#8b5cf6' },
		{ key: 'replyCount', label: '评论', color: '#06b6d4' },
		{ key: 'danmakuCount', label: '弹幕', color: '#ef4444' }
	];
	let metric = $derived(metrics.find((m) => m.key === selected)!);
	let points = $derived(
		(data?.stats ?? [])
			.filter((p) => p[selected] !== null)
			.map((p) => ({ time: new Date(p.recordedAt), value: p[selected]! }))
	);
	let chartConfig = $derived({
		value: { label: metric.label, color: metric.color }
	} satisfies Chart.ChartConfig);
	const format = (value: number | null | undefined) =>
		value == null ? '—' : value.toLocaleString('zh-CN');
	const growth = (value: number | null | undefined) =>
		value == null ? '暂无增长数据' : `${value > 0 ? '+' : ''}${format(value)}`;
	const date = (value: string) => new Date(value).toLocaleString('zh-CN');

	async function load(target: string, range: number | null) {
		const current = ++sequence;
		loading = true;
		error = null;
		data = null;
		try {
			const response = await api.getVideoStats(target, range);
			if (current === sequence) data = response.data;
		} catch (e) {
			if (current === sequence) error = (e as ApiError).message;
		} finally {
			if (current === sequence) loading = false;
		}
	}
	$effect(() => {
		void load(bvid, days);
	});
</script>

<section class="my-6 rounded-lg border p-4" aria-label="视频统计">
	<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
		<div>
			<h2 class="text-lg font-semibold">视频数据</h2>
			<p class="text-muted-foreground mt-1 text-xs">
				每次成功同步保留采样，增长按所选范围内的首末记录计算。
			</p>
		</div>
		<div class="flex flex-wrap items-center gap-1">
			{#each ranges as range (range.label)}
				<Button
					variant={days === range.days ? 'default' : 'outline'}
					aria-pressed={days === range.days}
					size="sm"
					class="h-7 px-2 text-xs"
					onclick={() => (days = range.days)}>{range.label}</Button
				>
			{/each}
			<Button
				variant="ghost"
				size="sm"
				aria-label="刷新视频统计"
				disabled={loading}
				onclick={() => load(bvid, days)}><RefreshCwIcon class="h-4 w-4" /></Button
			>
		</div>
	</div>
	{#if loading}
		<p class="text-muted-foreground py-10 text-center text-sm">正在加载视频统计…</p>
	{:else if error}
		<div class="text-destructive py-6 text-center text-sm" role="alert">
			加载失败：{error}<Button
				variant="outline"
				size="sm"
				class="ml-3"
				onclick={() => load(bvid, days)}>重试</Button
			>
		</div>
	{:else if data}
		{#if showTitle}
			<div class="mb-4 flex items-center gap-3">
				{#if data.cover}<img
						src={data.cover}
						alt="视频封面"
						referrerpolicy="no-referrer"
						class="h-16 w-28 rounded-md object-cover"
					/>{/if}
				<div class="min-w-0">
					<h3 class="font-medium">{data.title}</h3>
					<a
						href={`https://www.bilibili.com/video/${bvid}`}
						target="_blank"
						rel="noreferrer"
						class="text-muted-foreground text-xs hover:underline">{bvid} · 在 B 站打开</a
					>
				</div>
			</div>
		{/if}
		{#if data.latest}
			<div class="mb-4 grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-7">
				{#each metrics as m (m.key)}
					<button
						class="rounded-md border p-3 text-left transition-colors hover:bg-muted/50 {selected ===
						m.key
							? 'border-primary bg-primary/5'
							: ''}"
						aria-pressed={selected === m.key}
						onclick={() => (selected = m.key)}
					>
						<span class="text-muted-foreground text-xs">{m.label}</span>
						<span class="mt-1 block text-xl font-semibold tabular-nums"
							>{format(data.latest[m.key])}</span
						>
						<span
							class="mt-1 block text-xs tabular-nums {data.growth[m.key] === null
								? 'text-muted-foreground'
								: 'text-foreground'}">{growth(data.growth[m.key])}</span
						>
					</button>
				{/each}
			</div>
			<div class="mb-3 flex flex-wrap justify-between gap-2 text-xs text-muted-foreground">
				<span>{metric.label}趋势 · 范围内 {data.sampleCount.toLocaleString()} 次采样</span>
				<span>最近采样：{date(data.latest.recordedAt)}</span>
			</div>
			{#if points.length > 1}
				<Chart.Container config={chartConfig} class="h-[260px] w-full">
					<AreaChart
						data={points}
						x="time"
						axis="x"
						series={[{ key: 'value', label: metric.label, color: metric.color }]}
						props={{ area: { curve: curveLinear, line: { class: 'stroke-2' } } }}
					>
						{#snippet tooltip({ context })}<MyChartTooltip {context} indicator="line" />{/snippet}
					</AreaChart>
				</Chart.Container>
			{:else}
				<div class="text-muted-foreground flex h-[160px] items-center justify-center text-sm">
					{points.length === 1
						? '目前仅 1 个有效采样点，后续同步后会显示趋势与增长。'
						: '这个时间范围内暂无有效采样。'}
				</div>
			{/if}
			{#if data.baselineAt && data.sampleCount > 1}<p class="text-muted-foreground mt-3 text-xs">
					增长记录区间：{date(data.baselineAt)} 至 {date(data.latest.recordedAt)}
				</p>{/if}
			{#if data.sampleCount > data.stats.length}<p class="text-muted-foreground mt-1 text-xs">
					图表按采样顺序抽取 {data.stats.length} 个点，首末记录和原始快照均保留。
				</p>{/if}
		{:else}
			<p class="text-muted-foreground py-10 text-center text-sm">
				还没有视频统计快照，下一次同步会开始采集。
			</p>
		{/if}
		{#if data.relatedDynamics.length}
			<div class="mt-5 border-t pt-4">
				<h3 class="mb-2 text-sm font-medium">关联动态与评论</h3>
				<div class="flex flex-wrap gap-2">
					{#each data.relatedDynamics as dynamic (dynamic.id)}<a
							class="rounded-md border px-3 py-2 text-sm hover:bg-muted/50"
							href={`/dynamic-sources/${dynamic.sourceId}?dynamic=${dynamic.id}`}
							>{dynamic.upperName} · 本地有效评论 {dynamic.replyCount}</a
						>{/each}
				</div>
			</div>
		{/if}
	{/if}
</section>
