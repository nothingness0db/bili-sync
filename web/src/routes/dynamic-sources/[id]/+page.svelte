<script lang="ts">
	import { onDestroy } from 'svelte';
	import { afterNavigate } from '$app/navigation';
	import { page } from '$app/stores';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import Pagination from '$lib/components/pagination.svelte';
	import DynamicDetailDialog from '$lib/components/dynamic-detail-dialog.svelte';
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
	import type { ApiError, DynamicListItem, DynamicStatsResponse, StatPoint } from '$lib/types';
	import api from '$lib/api';
	import SourceVideoStats from '$lib/components/source-video-stats.svelte';
	import SourceElecStats from '$lib/components/source-elec-stats.svelte';

	let sourceId = 0;
	let dataSequence = 0;
	let lastDetailTarget = '';
	let loadError: string | null = null;
	let stats: DynamicStatsResponse | null = null;
	let statsRevision = 0;
	let dynamics: DynamicListItem[] = [];
	let dynamicPage = 0;
	let dynamicTotalCount = 0;
	const dynamicPageSize = 20;
	let loading = false;
	let rescanningAll = false;
	let scanningProfile = false;
	let syncingNow = false;
	let showSyncWarningDialog = false;
	let rescanningIds = new SvelteSet<string>();
	// 时间范围过滤（天），null = 全部
	let rangeDays: number | null = null;
	const RANGE_OPTIONS = [
		{ label: '全部', days: null },
		{ label: '90天', days: 90 },
		{ label: '30天', days: 30 },
		{ label: '7天', days: 7 }
	] as const;

	// 按时间范围过滤数据点（天），null = 全部；并按天聚合成日粒度
	function filteredStats(): StatPoint[] {
		const all = stats?.stats ?? [];
		let pts = all;
		if (rangeDays !== null) {
			const cutoff = Date.now() - rangeDays * 24 * 3600 * 1000;
			pts = all.filter((p) => new Date(p.recordedAt).getTime() >= cutoff);
		}
		const byDay: Record<string, StatPoint> = {};
		for (const p of pts) {
			const d = new Date(p.recordedAt);
			byDay[`${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`] = p;
		}
		return Object.values(byDay).sort((a, b) => a.recordedAt.localeCompare(b.recordedAt));
	}

	// 图表 x 轴固定为所选时间范围（全部 = 数据范围）
	function chartXDomain(): [Date, Date] | undefined {
		if (rangeDays === null) return undefined;
		const last = stats?.stats[stats.stats.length - 1];
		const end = last ? new Date(last.recordedAt) : new Date();
		return [new Date(end.getTime() - rangeDays * 24 * 3600 * 1000), end];
	}

	let showDetailDialog = false;
	let detailDynamicId = '';
	function openDetail(dynId: string) {
		detailDynamicId = dynId;
		showDetailDialog = true;
	}

	// 指标配置（七个折线图），totalVideoCount 由后端计算（视频投稿 + 仅动态视频）
	const METRICS = [
		{ key: 'fanCount', label: '粉丝数', color: 'var(--primary)' },
		{ key: 'followCount', label: '关注数', color: '#22c55e' },
		{ key: 'videoCount', label: '投稿数', color: '#f59e0b' },
		{ key: 'dynamicVideoCount', label: '动态视频数', color: '#06b6d4' },
		{ key: 'totalVideoCount', label: '总视频数', color: '#ef4444' },
		{ key: 'viewCount', label: '总播放数', color: '#8b5cf6' },
		{ key: 'likeCount', label: '总获赞数', color: '#ec4899' }
	] as const;

	function buildChartData(points: StatPoint[], key: (typeof METRICS)[number]['key']) {
		return points.map((p) => ({
			time: new Date(p.recordedAt),
			value: p[key]
		}));
	}

	function latestMetricValue(key: (typeof METRICS)[number]['key']): number {
		const last = stats?.stats[stats.stats.length - 1];
		if (!last) return 0;
		return last[key];
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
		const current = ++dataSequence;
		const target = sourceId;
		loading = true;
		loadError = null;
		try {
			const [statsResponse, dynamicsResponse] = await Promise.all([
				api.getDynamicSourceStats(target),
				api.getDynamicSourceDynamics(target, dynamicPage, dynamicPageSize)
			]);
			if (current !== dataSequence) return;
			stats = statsResponse.data;
			statsRevision += 1;
			dynamics = dynamicsResponse.data.dynamics;
			dynamicTotalCount = dynamicsResponse.data.totalCount;
			setBreadcrumb([{ label: '动态源', href: '/dynamic-sources' }, { label: stats.upperName }]);
		} catch (error) {
			if (current !== dataSequence) return;
			loadError = (error as ApiError).message;
			toast.error('加载数据失败', {
				description: (error as ApiError).message
			});
		} finally {
			if (current === dataSequence) loading = false;
		}
	}

	async function loadDynamicPage(nextPage: number) {
		const current = ++dataSequence;
		dynamicPage = nextPage;
		loading = true;
		try {
			const response = await api.getDynamicSourceDynamics(sourceId, nextPage, dynamicPageSize);
			if (current !== dataSequence) return;
			dynamics = response.data.dynamics;
			dynamicTotalCount = response.data.totalCount;
		} catch (error) {
			if (current !== dataSequence) return;
			toast.error('加载动态列表失败', {
				description: (error as ApiError).message
			});
		} finally {
			if (current === dataSequence) loading = false;
		}
	}

	async function scanProfileNow() {
		scanningProfile = true;
		try {
			await api.scanProfile(sourceId);
			toast.success('扫描完成', {
				description: '已刷新可用的账号、视频和充电榜数据'
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

	async function confirmSyncNow() {
		showSyncWarningDialog = false;
		await syncNow();
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

	onDestroy(() => {
		dataSequence += 1;
	});
	afterNavigate(() => {
		const nextSourceId = Number($page.params.id);
		if (sourceId !== nextSourceId) {
			sourceId = nextSourceId;
			stats = null;
			dynamics = [];
			dynamicPage = 0;
			dynamicTotalCount = 0;
			showDetailDialog = false;
			lastDetailTarget = '';
			setBreadcrumb([{ label: '动态源', href: '/dynamic-sources' }, { label: '动态源详情' }]);
			if (Number.isSafeInteger(sourceId) && sourceId > 0) void loadData();
			else {
				dataSequence += 1;
				loading = false;
				loadError = '无效的动态源 ID';
			}
		}
		const dynamicId = $page.url.searchParams.get('dynamic');
		const target = `${sourceId}:${dynamicId ?? ''}`;
		if (target !== lastDetailTarget) {
			lastDetailTarget = target;
			if (dynamicId && Number.isSafeInteger(sourceId) && sourceId > 0) openDetail(dynamicId);
			else showDetailDialog = false;
		}
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
	{:else if loadError && !stats}
		<div role="alert" class="space-y-3 py-12 text-center">
			<p class="text-destructive">加载失败：{loadError}</p>
			<Button size="sm" variant="outline" onclick={loadData}>重试</Button>
			<Button size="sm" variant="outline" href="/dynamic-sources">返回动态源</Button>
		</div>
	{:else if stats}
		<!-- 账号信息 -->
		<div class="flex flex-wrap items-center justify-between gap-4">
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
			<Button size="sm" variant="outline" href="#elec-stats">充电榜</Button>
			<Button
				size="sm"
				variant="outline"
				onclick={() => (showSyncWarningDialog = true)}
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
		<div class="flex items-center justify-between">
			<span class="text-sm font-medium">账号数据趋势</span>
			<div class="flex items-center gap-1">
				{#each RANGE_OPTIONS as opt (opt.label)}
					<Button
						size="sm"
						variant={rangeDays === opt.days ? 'default' : 'ghost'}
						onclick={() => (rangeDays = opt.days)}
						class="h-7 px-2 text-xs"
					>
						{opt.label}
					</Button>
				{/each}
			</div>
		</div>
		<div class="grid gap-6 lg:grid-cols-2">
			{#each METRICS as metric (metric.key)}
				<div class="rounded-lg border p-4">
					<div class="mb-3 flex items-center justify-between">
						<span class="text-sm font-medium">{metric.label}</span>
						{#if stats.stats.length > 0}
							<span class="text-foreground/80 text-lg font-semibold">
								{latestMetricValue(metric.key).toLocaleString()}
							</span>
						{/if}
					</div>
					{#if filteredStats().length > 1}
						<Chart.Container
							config={chartConfig(metric.label, metric.color)}
							class="h-[150px] w-full"
						>
							<AreaChart
								data={buildChartData(filteredStats(), metric.key)}
								x="time"
								xDomain={chartXDomain()}
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
								{#snippet tooltip({ context })}
									<MyChartTooltip {context} indicator="line" />
								{/snippet}
							</AreaChart>
						</Chart.Container>
					{:else if filteredStats().length === 1}
						<div class="text-muted-foreground flex h-[150px] items-center justify-center text-sm">
							该时间范围内仅 1 条记录，需至少 2 条才能绘制趋势图
						</div>
					{:else}
						<div class="text-muted-foreground flex h-[150px] items-center justify-center text-sm">
							{rangeDays === null
								? '数据不足，等待记录（下一轮任务执行后）'
								: '该时间范围内暂无数据'}
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
		<SourceElecStats {sourceId} refreshKey={statsRevision} />
		<SourceVideoStats {sourceId} refreshKey={statsRevision} />

		<div class="rounded-lg border p-4">
			<div class="mb-3 flex items-center justify-between">
				<div class="flex items-center gap-2">
					<MessagesSquareIcon class="h-4 w-4" />
					<span class="text-sm font-medium">动态列表（评论自动同步至发布后 30 天）</span>
					<span class="text-muted-foreground text-xs"
						>1 天内每小时，2–7 天每 6 小时，8–30 天每 24 小时</span
					>
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
										{#if !dyn.valid}
											<Badge variant="secondary" class="flex w-fit items-center gap-1.5">
												已删除（本地保留）
											</Badge>
										{:else if dyn.rescanReply}
											<Badge class="flex w-fit items-center gap-1.5 bg-amber-600 text-amber-50">
												<RefreshCwIcon class="h-3 w-3" />
												{dyn.replySyncStarted ? '等待续抓' : '等待重扫'}
											</Badge>
										{:else if !dyn.path}
											<Badge class="flex w-fit items-center gap-1.5 bg-rose-700 text-rose-100">
												待处理
											</Badge>
										{:else if !dyn.replySyncedAt && dyn.commentCount > 0 && dyn.replyCount === 0}
											<Badge class="flex w-fit items-center gap-1.5 bg-amber-600 text-amber-50">
												<RefreshCwIcon class="h-3 w-3" />
												评论待补拉
											</Badge>
										{:else if !dyn.replySyncedAt && dyn.commentCount > 0 && dyn.replyCount < dyn.commentCount}
											<Badge variant="secondary" class="flex w-fit items-center gap-1.5">
												评论部分同步（{dyn.replyCount}/{dyn.commentCount}）
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
				<Pagination
					currentPage={dynamicPage}
					totalPages={Math.ceil(dynamicTotalCount / dynamicPageSize)}
					onPageChange={loadDynamicPage}
				/>
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

	<AlertDialog.Root bind:open={showSyncWarningDialog}>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>确认立即同步？</AlertDialog.Title>
				<AlertDialog.Description>
					将同步该动态源的账号数据、动态和评论。若该源正在同步，本次操作会排队等待。进度可在任务看板和日志中查看。
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel disabled={syncingNow}>取消</AlertDialog.Cancel>
				<AlertDialog.Action onclick={confirmSyncNow} disabled={syncingNow}>
					{syncingNow ? '触发中...' : '确认手动同步'}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>

	<DynamicDetailDialog bind:open={showDetailDialog} {sourceId} dynamicId={detailDynamicId} />
</div>
