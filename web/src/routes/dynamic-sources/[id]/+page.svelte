<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Chart from '$lib/components/ui/chart/index.js';
	import MyChartTooltip from '$lib/components/custom/my-chart-tooltip.svelte';
	import { AreaChart } from 'layerchart';
	import { curveNatural } from 'd3-shape';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import PlayIcon from '@lucide/svelte/icons/play';
	import HistoryIcon from '@lucide/svelte/icons/history';
	import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
	import ScanSearchIcon from '@lucide/svelte/icons/scan-search';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import UserIcon from '@lucide/svelte/icons/user';
	import { toast } from 'svelte-sonner';
	import { SvelteSet } from 'svelte/reactivity';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import type {
		ApiError,
		DynamicDetailResponse,
		DynamicListItem,
		DynamicStatsResponse,
		ReplyItem,
		StatPoint
	} from '$lib/types';
	import api from '$lib/api';

	let sourceId = Number(page.params.id);
	let stats: DynamicStatsResponse | null = null;
	let dynamics: DynamicListItem[] = [];
	let loading = false;
	let rescanningAll = false;
	let scanningProfile = false;
	let syncingNow = false;
	let rescanningIds = new SvelteSet<string>();

	// 动态详情对话框
	let showDetailDialog = false;
	let detailLoading = false;
	let detail: DynamicDetailResponse | null = null;

	async function openDetail(dynId: string) {
		showDetailDialog = true;
		detailLoading = true;
		detail = null;
		try {
			const response = await api.getDynamicDetail(sourceId, dynId);
			detail = response.data;
		} catch (error) {
			toast.error('加载动态详情失败', {
				description: (error as ApiError).message
			});
			showDetailDialog = false;
		} finally {
			detailLoading = false;
		}
	}

	function statCount(stat: Record<string, unknown> | undefined, key: string): number {
		if (!stat) return 0;
		const v = stat[key] as { count?: number } | undefined;
		return v?.count ?? 0;
	}

	// 指标配置（五个折线图）
	const METRICS = [
		{ key: 'fanCount', label: '粉丝数', color: 'var(--primary)' },
		{ key: 'followCount', label: '关注数', color: '#22c55e' },
		{ key: 'videoCount', label: '投稿数', color: '#f59e0b' },
		{ key: 'viewCount', label: '总播放数', color: '#8b5cf6' },
		{ key: 'likeCount', label: '总获赞数', color: '#ec4899' }
	] as const;

	function buildChartData(points: StatPoint[], key: (typeof METRICS)[number]['key']) {
		return points.map((p) => ({
			time: new Date(p.recordedAt).toLocaleString('zh-CN', {
				month: '2-digit',
				day: '2-digit',
				hour: '2-digit',
				minute: '2-digit'
			}),
			value: p[key]
		}));
	}

	function chartConfig(label: string, color: string) {
		return {
			value: {
				label,
				color
			}
		} satisfies Chart.ChartConfig;
	}

	async function loadData() {
		loading = true;
		try {
			const [statsResponse, dynamicsResponse] = await Promise.all([
				api.getDynamicSourceStats(sourceId),
				api.getDynamicSourceDynamics(sourceId)
			]);
			stats = statsResponse.data;
			dynamics = dynamicsResponse.data;
			setBreadcrumb([{ label: '动态源', href: '/dynamic-sources' }, { label: stats.upperName }]);
		} catch (error) {
			toast.error('加载数据失败', {
				description: (error as ApiError).message
			});
		} finally {
			loading = false;
		}
	}

	async function scanProfileNow() {
		scanningProfile = true;
		try {
			await api.scanProfile(sourceId);
			toast.success('扫描完成', {
				description: '账号数据已刷新（有变化才会生成新记录）'
			});
			await loadData();
		} catch (error) {
			toast.error('扫描失败', {
				description: (error as ApiError).message
			});
		} finally {
			scanningProfile = false;
		}
	}

	async function syncNow() {
		syncingNow = true;
		try {
			await api.syncNow(sourceId);
			toast.success('已触发同步', {
				description: '后台立即执行一轮完整同步（账号数据 + 动态 + 评论），进度见日志页'
			});
		} catch (error) {
			toast.error('触发失败', {
				description: (error as ApiError).message
			});
		} finally {
			syncingNow = false;
		}
	}

	async function rescanAll() {
		rescanningAll = true;
		try {
			const response = await api.rescanAllReplies(sourceId);
			toast.success('已标记重扫', {
				description: `${response.data} 条动态将在下一轮任务中重新同步评论`
			});
			await loadData();
		} catch (error) {
			toast.error('标记失败', {
				description: (error as ApiError).message
			});
		} finally {
			rescanningAll = false;
		}
	}

	async function rescanSingle(dynId: string) {
		rescanningIds.add(dynId);
		try {
			await api.rescanSingleReply(sourceId, dynId);
			toast.success('已标记重扫', {
				description: '该动态将在下一轮任务中重新同步评论'
			});
			await loadData();
		} catch (error) {
			toast.error('标记失败', {
				description: (error as ApiError).message
			});
		} finally {
			rescanningIds.delete(dynId);
		}
	}

	onMount(() => {
		loadData();
	});
</script>

<svelte:head>
	<title>动态源详情 - Bili Sync</title>
</svelte:head>

<div class="space-y-6">
	{#if loading && !stats}
		<div class="flex items-center justify-center py-12">
			<div class="text-muted-foreground">加载中...</div>
		</div>
	{:else if stats}
		<!-- 账号信息 -->
		<div class="flex items-center justify-between gap-4">
			<div class="flex items-center gap-4">
				{#if stats.versions.length > 0}
					<img
						src={stats.versions[stats.versions.length - 1].face}
						alt="avatar"
						class="h-14 w-14 rounded-full"
					/>
				{:else}
					<div class="bg-muted flex h-14 w-14 items-center justify-center rounded-full">
						<UserIcon class="text-muted-foreground h-6 w-6" />
					</div>
				{/if}
				<div>
					<div class="flex items-center gap-2">
						<span class="text-xl font-semibold">{stats.upperName}</span>
						<span class="text-muted-foreground text-sm">({stats.upperId})</span>
					</div>
					<div class="text-muted-foreground text-sm">
						{stats.versions[stats.versions.length - 1]?.sign || '这个人很懒，什么都没写'}
					</div>
				</div>
			</div>
			<Button
				size="sm"
				variant="outline"
				onclick={syncNow}
				disabled={syncingNow}
				class="flex items-center gap-2"
			>
				<PlayIcon class="h-3.5 w-3.5" />
				{syncingNow ? '触发中...' : '立即同步'}
			</Button>
			<Button
				size="sm"
				variant="outline"
				onclick={scanProfileNow}
				disabled={scanningProfile}
				class="flex items-center gap-2"
			>
				<ScanSearchIcon class="h-3.5 w-3.5" />
				{scanningProfile ? '扫描中...' : '立即扫描账号数据'}
			</Button>
		</div>

		<!-- 数据折线图 -->
		<div class="grid gap-6 lg:grid-cols-2">
			{#each METRICS as metric (metric.key)}
				<div class="rounded-lg border p-4">
					<div class="mb-3 flex items-center justify-between">
						<span class="text-sm font-medium">{metric.label}</span>
						{#if stats.stats.length > 0}
							<span class="text-foreground/80 text-lg font-semibold">
								{stats.stats[stats.stats.length - 1][metric.key].toLocaleString()}
							</span>
						{/if}
					</div>
					{#if stats.stats.length > 1}
						<Chart.Container
							config={chartConfig(metric.label, metric.color)}
							class="h-[150px] w-full"
						>
							<AreaChart
								data={buildChartData(stats.stats, metric.key)}
								x="time"
								axis="x"
								series={[
									{
										key: 'value',
										label: metric.label,
										color: metric.color
									}
								]}
								props={{
									area: {
										curve: curveNatural,
										line: { class: 'stroke-1' }
									}
								}}
							>
								{#snippet tooltip()}
									<MyChartTooltip indicator="line" />
								{/snippet}
							</AreaChart>
						</Chart.Container>
					{:else}
						<div class="text-muted-foreground flex h-[150px] items-center justify-center text-sm">
							数据不足，等待记录（下一轮任务执行后）
						</div>
					{/if}
				</div>
			{/each}
		</div>

		<!-- 名字/签名版本历史 -->
		<div class="rounded-lg border p-4">
			<div class="mb-3 flex items-center gap-2">
				<HistoryIcon class="h-4 w-4" />
				<span class="text-sm font-medium">名字 / 签名版本历史</span>
			</div>
			{#if stats.versions.length > 0}
				<div class="space-y-3">
					{#each [...stats.versions].reverse() as version, i (i)}
						<div class="flex items-start gap-3">
							<img src={version.face} alt="avatar" class="mt-0.5 h-8 w-8 rounded-full" />
							<div class="flex-1">
								<div class="flex items-center gap-2">
									<span class="font-medium">{version.name}</span>
									<Badge variant="secondary" class="text-xs">
										{new Date(version.startAt).toLocaleString('zh-CN')}
									</Badge>
									{#if version.endAt}
										<span class="text-muted-foreground text-xs">
											→ {new Date(version.endAt).toLocaleString('zh-CN')}
										</span>
									{:else}
										<span class="text-muted-foreground text-xs">→ 至今</span>
									{/if}
								</div>
								<div class="text-muted-foreground mt-0.5 text-sm">
									{version.sign || '（无签名）'}
								</div>
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<div class="text-muted-foreground py-4 text-center text-sm">暂无记录</div>
			{/if}
		</div>

		<!-- 动态列表（手动重扫评论） -->
		<div class="rounded-lg border p-4">
			<div class="mb-3 flex items-center justify-between">
				<div class="flex items-center gap-2">
					<MessagesSquareIcon class="h-4 w-4" />
					<span class="text-sm font-medium">动态列表（评论自动同步发布后 5 天）</span>
				</div>
				<Button
					size="sm"
					variant="outline"
					onclick={rescanAll}
					disabled={rescanningAll}
					class="flex items-center gap-2"
				>
					<RefreshCwIcon class="h-3.5 w-3.5" />
					{rescanningAll ? '标记中...' : '全部重扫评论'}
				</Button>
			</div>
			{#if dynamics.length > 0}
				<div class="overflow-x-auto">
					<Table.Root>
						<Table.Header>
							<Table.Row>
								<Table.Head class="w-[15%]">发布时间</Table.Head>
								<Table.Head class="w-[18%]">类型</Table.Head>
								<Table.Head>内容</Table.Head>
								<Table.Head class="w-[8%]">评论数</Table.Head>
								<Table.Head class="w-[12%]">状态</Table.Head>
								<Table.Head class="w-[10%] text-right">操作</Table.Head>
							</Table.Row>
						</Table.Header>
						<Table.Body>
							{#each dynamics as dyn (dyn.id)}
								<Table.Row>
									<Table.Cell>
										{new Date(dyn.pubTs).toLocaleString('zh-CN')}
									</Table.Cell>
									<Table.Cell>
										<Badge variant="secondary" class="font-mono text-xs">
											{dyn.dynType.replace('DYNAMIC_TYPE_', '')}
										</Badge>
									</Table.Cell>
									<Table.Cell class="max-w-[300px]">
										<span class="text-muted-foreground line-clamp-1 text-sm">
											{dyn.content || '（无正文）'}
										</span>
									</Table.Cell>
									<Table.Cell>
										<Badge variant="secondary" class="flex w-fit items-center gap-1">
											<MessagesSquareIcon class="h-3 w-3" />
											{dyn.commentCount}
										</Badge>
									</Table.Cell>
									<Table.Cell>
										{#if dyn.rescanReply}
											<Badge class="flex w-fit items-center gap-1.5 bg-amber-600 text-amber-50">
												<RefreshCwIcon class="h-3 w-3" />
												等待重扫
											</Badge>
										{:else if !dyn.path}
											<Badge class="flex w-fit items-center gap-1.5 bg-rose-700 text-rose-100">
												待处理
											</Badge>
										{:else if dyn.commentCount > 0 && dyn.replyCount < dyn.commentCount}
											<Badge class="flex w-fit items-center gap-1.5 bg-amber-600 text-amber-50">
												<RefreshCwIcon class="h-3 w-3" />
												评论待补拉（{dyn.replyCount}/{dyn.commentCount}）
											</Badge>
										{:else}
											<Badge
												class="flex w-fit items-center gap-1.5 bg-emerald-700 text-emerald-100"
											>
												已同步
											</Badge>
										{/if}
									</Table.Cell>
									<Table.Cell class="text-right">
										<Tooltip.Root disableHoverableContent={true}>
											<Tooltip.Trigger>
												<Button
													size="sm"
													variant="outline"
													onclick={() => openDetail(dyn.id)}
													class="h-8 w-8 p-0"
												>
													<EyeIcon class="h-3 w-3" />
												</Button>
											</Tooltip.Trigger>
											<Tooltip.Content>
												<p class="text-xs">查看正文与评论</p>
											</Tooltip.Content>
										</Tooltip.Root>
										<Tooltip.Root disableHoverableContent={true}>
											<Tooltip.Trigger>
												<Button
													size="sm"
													variant="outline"
													onclick={() => rescanSingle(dyn.id)}
													disabled={rescanningIds.has(dyn.id)}
													class="h-8 w-8 p-0"
												>
													<RefreshCwIcon class="h-3 w-3" />
												</Button>
											</Tooltip.Trigger>
											<Tooltip.Content>
												<p class="text-xs">重扫该动态的评论</p>
											</Tooltip.Content>
										</Tooltip.Root>
									</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</div>
			{:else}
				<div class="text-muted-foreground py-8 text-center text-sm">暂无动态</div>
			{/if}
		</div>
	{:else}
		<div class="flex flex-col items-center justify-center py-12">
			<div class="text-muted-foreground mb-2">加载失败</div>
			<Button class="mt-4" onclick={loadData}>重新加载</Button>
		</div>
	{/if}

	<!-- 动态详情对话框：正文 + 评论树 -->
	<Dialog.Root bind:open={showDetailDialog}>
		<Dialog.Content
			class="no-scrollbar max-h-[85vh] max-w-[90vw]! overflow-y-auto lg:max-w-[60vw]!"
		>
			<Dialog.Title class="text-lg font-semibold">动态详情</Dialog.Title>
			{#if detailLoading}
				<div class="flex items-center justify-center py-12">
					<div class="text-muted-foreground">加载中...</div>
				</div>
			{:else if detail}
				<div class="mt-4 space-y-4">
					<!-- 动态元信息 -->
					<div class="flex flex-wrap items-center gap-2 text-sm">
						<Badge variant="secondary" class="font-mono text-xs">
							{detail.dynType.replace('DYNAMIC_TYPE_', '')}
						</Badge>
						<span class="text-muted-foreground">
							{new Date(detail.pubTs).toLocaleString('zh-CN')}
						</span>
						<Badge variant="secondary" class="flex items-center gap-1">
							<MessagesSquareIcon class="h-3 w-3" />
							评论 {statCount(detail.stat, 'comment')}
						</Badge>
						<Badge variant="secondary">赞 {statCount(detail.stat, 'like')}</Badge>
						<Badge variant="secondary">转 {statCount(detail.stat, 'forward')}</Badge>
						{#if detail.location}
							<Badge variant="secondary">📍 {detail.location}</Badge>
						{/if}
						<span class="text-muted-foreground font-mono text-xs">{detail.id}</span>
					</div>
					<!-- 正文 -->
					{#if detail.content}
						<div class="bg-muted/50 rounded-lg p-4 text-sm leading-relaxed whitespace-pre-wrap">
							{detail.content}
						</div>
					{:else}
						<div class="text-muted-foreground text-sm">（该动态无正文文本）</div>
					{/if}
					<!-- 图片 -->
					{#if detail.pics.length > 0}
						<div>
							<div class="mb-2 text-sm font-medium">图片（{detail.pics.length}）</div>
							<div class="flex flex-wrap gap-3">
								{#each detail.pics as _, i (i)}
									<img
										src={`/api/dynamic-sources/${sourceId}/dynamics/${detail.id}/file?name=pics/${String(i + 1).padStart(2, '0')}.jpg`}
										alt={`图片 ${i + 1}`}
										class="max-h-64 max-w-full rounded-lg border object-contain"
									/>
								{/each}
							</div>
						</div>
					{/if}
					<!-- 本地文件路径 -->
					{#if detail.path}
						<div class="text-muted-foreground font-mono text-xs">
							落盘位置: {detail.path}
						</div>
					{/if}
					<!-- 评论树 -->
					<div>
						<div class="mb-2 flex items-center gap-2">
							<MessagesSquareIcon class="h-4 w-4" />
							<span class="text-sm font-medium">评论（{detail.replies.length}）</span>
						</div>
						{#if detail.replies.length > 0}
							<div class="space-y-4">
								{#each detail.replies as reply (reply.rpid)}
									{@render ReplyRow({ reply })}
								{/each}
							</div>
						{:else}
							<div class="text-muted-foreground py-6 text-center text-sm">暂无评论</div>
						{/if}
					</div>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>
</div>

{#snippet ReplyRow({ reply, depth = 0 }: { reply: ReplyItem; depth?: number })}
	<div class="flex gap-3" style="margin-left: {depth * 24}px">
		{#if reply.avatar}
			<img src={reply.avatar} alt={reply.uname} class="h-8 w-8 shrink-0 rounded-full" />
		{:else}
			<div class="bg-muted flex h-8 w-8 shrink-0 items-center justify-center rounded-full">
				<UserIcon class="text-muted-foreground h-4 w-4" />
			</div>
		{/if}
		<div class="min-w-0 flex-1">
			<div class="flex flex-wrap items-center gap-2 text-xs">
				<span class="font-medium">{reply.uname}</span>
				<span class="text-muted-foreground">
					{new Date(reply.ctime).toLocaleString('zh-CN')}
				</span>
				{#if reply.parentRpid}
					<Badge variant="secondary" class="text-[10px]">楼中楼</Badge>
				{/if}
			</div>
			<div class="text-muted-foreground mt-1 text-sm break-words">
				{reply.content || '（无内容）'}
			</div>
			{#if reply.images.length > 0}
				<div class="mt-1 flex flex-wrap gap-2">
					{#each reply.images as _, i (i)}
						<img
							src={`/api/dynamic-sources/${sourceId}/dynamics/${detail!.id}/file?name=comments/${reply.rpid}_${i + 1}.jpg`}
							alt="评论图片"
							class="h-24 max-w-48 rounded-md border object-cover"
						/>
					{/each}
				</div>
			{/if}
			{#if reply.subReplies.length > 0}
				<div class="mt-2 space-y-3">
					{#each reply.subReplies as sub (sub.rpid)}
						{@render ReplyRow({ reply: sub, depth: depth + 1 })}
					{/each}
				</div>
			{/if}
		</div>
	</div>
{/snippet}
