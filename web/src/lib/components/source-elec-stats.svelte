<script lang="ts">
	import api from '$lib/api';
	import type { ApiError, ElecHistory, ElecPoint, ElecStatsResponse } from '$lib/types';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import * as Chart from '$lib/components/ui/chart/index.js';
	import MyChartTooltip from '$lib/components/custom/my-chart-tooltip.svelte';
	import Pagination from '$lib/components/pagination.svelte';
	import { AreaChart } from 'layerchart';
	import { curveStepAfter } from 'd3-shape';

	let { sourceId, refreshKey = 0 }: { sourceId: number; refreshKey?: number } = $props();
	let data = $state<ElecStatsResponse | null>(null);
	let error = $state<string | null>(null);
	let loading = $state(false);
	let days = $state<number | null>(7);
	let currentPage = $state(0);
	let selected = $state<ElecHistory | null>(null);
	let metric = $state<'total' | 'listedCount'>('total');
	let sequence = 0;
	const ranges = [
		{ label: '7天', days: 7 },
		{ label: '30天', days: 30 },
		{ label: '90天', days: 90 },
		{ label: '全部', days: null }
	];
	const date = (value: string) => new Date(value).toLocaleString('zh-CN');
	const total = (point: ElecPoint) =>
		point.upowerCountShow === false || point.total === null
			? '—'
			: point.total.toLocaleString('zh-CN');
	const stateLabel = (point: ElecPoint) =>
		point.state === -1
			? '未开启充电'
			: !point.show
				? '充电未展示'
				: point.state === 3
					? '开放且有榜'
					: point.state === 1
						? '开放，当前无榜'
						: `状态 ${point.state}`;
	let board = $derived(selected?.snapshot ?? data?.latest ?? null);
	let label = $derived(metric === 'total' ? '累计充电人数' : '接口在榜人数');
	let points = $derived(
		(data?.points ?? [])
			.filter((p) => p[metric] !== null && (metric !== 'total' || p.upowerCountShow !== false))
			.map((p) => ({ time: new Date(p.recordedAt), value: p[metric]! }))
	);
	let chartConfig = $derived({
		value: { label, color: 'var(--primary)' }
	} satisfies Chart.ChartConfig);

	async function load(id: number, page: number, range: number | null) {
		const current = ++sequence;
		loading = true;
		error = null;
		try {
			const response = await api.getElecStats(id, page, range);
			if (current === sequence) {
				data = response.data;
				selected = null;
			}
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

<section id="elec-stats" class="scroll-mt-4 rounded-lg border p-4" aria-label="充电榜历史">
	<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
		<div>
			<h2 class="text-sm font-medium">充电榜历史</h2>
			<p class="text-muted-foreground mt-1 text-xs">
				累计人数与榜单分别采样，榜单仅包含接口返回的前几名。
			</p>
		</div>
		<div class="flex flex-wrap gap-1">
			{#each ranges as range (range.label)}<Button
					size="sm"
					class="h-7 px-2 text-xs"
					variant={days === range.days ? 'default' : 'outline'}
					aria-pressed={days === range.days}
					onclick={() => {
						days = range.days;
						currentPage = 0;
					}}>{range.label}</Button
				>{/each}
			<Button
				size="sm"
				variant="outline"
				class="h-7 px-2 text-xs"
				disabled={loading}
				onclick={() => load(sourceId, currentPage, days)}>刷新充电榜</Button
			>
		</div>
	</div>
	{#if loading}<p class="text-muted-foreground py-6 text-center text-sm">正在加载充电榜…</p>
	{:else if error}<p role="alert" class="text-destructive py-4 text-sm">
			加载失败：{error}<Button
				class="ml-3"
				variant="outline"
				size="sm"
				onclick={() => load(sourceId, currentPage, days)}>重试</Button
			>
		</p>
	{:else if data?.latest}
		<div class="mb-4 grid gap-3 sm:grid-cols-3">
			<div class="rounded-md border p-3">
				<span class="text-muted-foreground text-xs">累计充电人数（历史）</span>
				<p class="mt-1 text-2xl font-semibold tabular-nums">{total(data.latest)}</p>
				<p class="text-muted-foreground mt-1 text-xs">
					{data.latest.upowerCountShow === false
						? '平台未展示人数'
						: data.totalGrowth === null
							? '暂无增长数据'
							: `记录期间 ${data.totalGrowth > 0 ? '+' : ''}${data.totalGrowth.toLocaleString('zh-CN')}`}
				</p>
			</div>
			<div class="rounded-md border p-3">
				<span class="text-muted-foreground text-xs">接口在榜人数</span>
				<p class="mt-1 text-2xl font-semibold tabular-nums">{data.latest.listedCount ?? '—'}</p>
				<p class="text-muted-foreground mt-1 text-xs">不代表全部活跃充电用户</p>
			</div>
			<div class="rounded-md border p-3">
				<span class="text-muted-foreground text-xs">最近充电状态</span>
				<p class="mt-2 font-medium">{stateLabel(data.latest)}</p>
				<p class="text-muted-foreground mt-2 text-xs">{date(data.latest.recordedAt)}</p>
			</div>
		</div>
		<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
			<p class="text-muted-foreground text-xs">{label}趋势 · 范围内 {data.sampleCount} 次采样</p>
			<div class="flex gap-1">
				<Button
					size="sm"
					class="h-7 text-xs"
					variant={metric === 'total' ? 'default' : 'outline'}
					aria-pressed={metric === 'total'}
					onclick={() => (metric = 'total')}>累计人数</Button
				><Button
					size="sm"
					class="h-7 text-xs"
					variant={metric === 'listedCount' ? 'default' : 'outline'}
					aria-pressed={metric === 'listedCount'}
					onclick={() => (metric = 'listedCount')}>在榜人数</Button
				>
			</div>
		</div>
		{#if points.length > 1}<Chart.Container config={chartConfig} class="h-[200px] w-full"
				><AreaChart
					data={points}
					x="time"
					axis="x"
					series={[{ key: 'value', label, color: 'var(--primary)' }]}
					props={{ area: { curve: curveStepAfter, line: { class: 'stroke-2' } } }}
					>{#snippet tooltip({ context })}<MyChartTooltip
							{context}
							indicator="line"
						/>{/snippet}</AreaChart
				></Chart.Container
			>
		{:else}<p class="text-muted-foreground py-6 text-center text-sm">
				{points.length === 1
					? '目前仅 1 个有效采样点，后续同步后会显示趋势。'
					: '这个范围内暂无有效人数采样。'}
			</p>{/if}
		{#if data.baselineAt && data.sampleCount > 1}<p class="text-muted-foreground mt-2 text-xs">
				增长记录区间：{date(data.baselineAt)} 至 {date(data.latest.recordedAt)}
			</p>{/if}
		{#if data.sampleCount > data.points.length}<p class="text-muted-foreground mt-1 text-xs">
				图表抽取 {data.points.length} 个点，原始榜单快照全部保留。
			</p>{/if}
		{#if board}
			<div class="mt-5 border-t pt-4">
				<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
					<h3 class="text-sm font-medium">
						{selected ? '历史榜单' : '最近榜单'} · {date(board.recordedAt)}
					</h3>
					{#if selected}<Button
							size="sm"
							variant="ghost"
							class="h-7 text-xs"
							onclick={() => (selected = null)}>返回最近榜单</Button
						>{/if}
				</div>
				{#if !board.listAvailable}<p class="text-muted-foreground text-sm">
						{stateLabel(board)}，本次榜单不可观察，无法确认成员变化。
					</p>
				{:else if !board.members.length}<p class="text-muted-foreground text-sm">
						本次接口返回空榜。
					</p>
				{:else}<div class="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
						{#each board.members as member (member.payMid)}<a
								class="flex items-center gap-3 rounded-md border p-3 hover:bg-muted/50"
								href={`https://space.bilibili.com/${member.payMid}`}
								target="_blank"
								rel="noreferrer"
								><span class="text-muted-foreground w-5 text-sm tabular-nums">#{member.rank}</span
								>{#if member.avatar}<img
										src={member.avatar}
										alt=""
										referrerpolicy="no-referrer"
										class="h-8 w-8 rounded-full"
									/>{/if}<span class="min-w-0"
									><span class="block truncate text-sm font-medium"
										>{member.uname || `UID ${member.payMid}`}</span
									><span class="text-muted-foreground text-xs">UID {member.payMid}</span></span
								></a
							>{/each}
					</div>{/if}
				{#if selected}<div class="mt-3 text-sm">
						{#if selected.comparisonAvailable}<p class="text-muted-foreground mb-2 text-xs">
								相较于 {date(selected.previousAt!)} 的榜单
							</p>
							<p>
								进入榜单：{selected.entered.map((m) => m.uname || `UID ${m.payMid}`).join('、') ||
									'无'}
							</p>
							<p class="mt-1">
								离开榜单：{selected.left.map((m) => m.uname || `UID ${m.payMid}`).join('、') ||
									'无'}
							</p>
							{#each selected.rankChanges as change (change.payMid)}<p class="mt-1">
									{change.uname || `UID ${change.payMid}`}：#{change.fromRank} → #{change.toRank}
								</p>{/each}{:else}<p class="text-muted-foreground text-xs">
								没有可比较的相邻榜单，无法确认进出榜。
							</p>{/if}
					</div>{/if}
			</div>
		{/if}
		<div class="mt-5 overflow-x-auto">
			<Table.Root
				><Table.Header
					><Table.Row
						><Table.Head>采样时间</Table.Head><Table.Head>累计人数</Table.Head><Table.Head
							>接口在榜</Table.Head
						><Table.Head>榜单变化</Table.Head><Table.Head class="text-right">查看</Table.Head
						></Table.Row
					></Table.Header
				><Table.Body
					>{#each data.history as item (item.snapshot.id)}<Table.Row
							class={selected?.snapshot.id === item.snapshot.id ? 'bg-muted/50' : ''}
							><Table.Cell class="whitespace-nowrap text-xs"
								>{date(item.snapshot.recordedAt)}</Table.Cell
							><Table.Cell class="tabular-nums">{total(item.snapshot)}</Table.Cell><Table.Cell
								class="tabular-nums">{item.snapshot.listedCount ?? '—'}</Table.Cell
							><Table.Cell class="whitespace-nowrap text-xs"
								>{item.comparisonAvailable
									? `进榜 ${item.entered.length} · 离榜 ${item.left.length} · 名次变化 ${item.rankChanges.length}`
									: '无可比榜单'}</Table.Cell
							><Table.Cell class="text-right"
								><Button
									size="sm"
									variant="outline"
									class="h-7 text-xs"
									aria-pressed={selected?.snapshot.id === item.snapshot.id}
									aria-label={`查看 ${date(item.snapshot.recordedAt)} 的充电榜`}
									onclick={() => (selected = item)}>查看榜单</Button
								></Table.Cell
							></Table.Row
						>{/each}</Table.Body
				></Table.Root
			>
		</div>
		<Pagination
			{currentPage}
			totalPages={Math.ceil(data.sampleCount / 20)}
			onPageChange={(page) => (currentPage = page)}
		/>
		<p class="text-muted-foreground mt-3 text-xs">
			进出榜仅表示返回的 Top 榜变化，不能据此判断续费或停充；金额和流水不在此接口中。
		</p>
	{:else}<p class="text-muted-foreground py-6 text-center text-sm">
			尚无充电榜快照，下一次账号同步会开始采集。
		</p>{/if}
</section>
