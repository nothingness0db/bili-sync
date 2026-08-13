<script lang="ts">
	import { onMount } from 'svelte';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js';
	import { Progress } from '$lib/components/ui/progress/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Chart from '$lib/components/ui/chart/index.js';
	import MyChartTooltip from '$lib/components/custom/my-chart-tooltip.svelte';
	import { curveNatural } from 'd3-shape';
	import { BarChart, AreaChart } from 'layerchart';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import { toast } from 'svelte-sonner';
	import * as Collapsible from '$lib/components/ui/collapsible/index.js';
	import CloudDownloadIcon from '@lucide/svelte/icons/cloud-download';
	import api from '$lib/api';
	import type {
		DashBoardResponse,
		SysInfo,
		ApiError,
		TaskStatus,
		TaskBoardResponse
	} from '$lib/types';
	import CalendarIcon from '@lucide/svelte/icons/calendar';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import CircleCheckBigIcon from '@lucide/svelte/icons/circle-check-big';
	import CircleIcon from '@lucide/svelte/icons/circle';
	import ClockIcon from '@lucide/svelte/icons/clock';
	import CpuIcon from '@lucide/svelte/icons/cpu';
	import DatabaseIcon from '@lucide/svelte/icons/database';
	import DownloadIcon from '@lucide/svelte/icons/download';
	import FolderIcon from '@lucide/svelte/icons/folder';
	import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
	import HeartIcon from '@lucide/svelte/icons/heart';
	import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
	import MemoryStickIcon from '@lucide/svelte/icons/memory-stick';
	import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
	import PlayIcon from '@lucide/svelte/icons/play';
	import RadarIcon from '@lucide/svelte/icons/radar';
	import ScanSearchIcon from '@lucide/svelte/icons/scan-search';
	import UserIcon from '@lucide/svelte/icons/user';
	import VideoIcon from '@lucide/svelte/icons/video';

	let dashboardData = $state<DashBoardResponse | null>(null);
	let sysInfo = $state<SysInfo | null>(null);
	let taskStatus = $state<TaskStatus | null>(null);
	let taskBoard = $state<TaskBoardResponse | null>(null);
	let taskBoardOpen = $state(true);
	let loading = $state(false);
	let triggering = $state(false);
	let memoryHistory = $state<Array<{ time: number; used: number; process: number }>>([]);
	let cpuHistory = $state<Array<{ time: number; used: number; process: number }>>([]);
	let unsubscribeSysInfo: (() => void) | null = null;
	let unsubscribeTasks: (() => void) | null = null;
	let taskBoardTimer: ReturnType<typeof setInterval> | null = null;

	function formatBytes(bytes: number): string {
		if (bytes === 0) return '0 B';
		const k = 1024;
		const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}

	function formatCpu(cpu: number): string {
		return `${cpu.toFixed(1)}%`;
	}

	function formatTimestamp(timestamp: number): string {
		return new Date(timestamp).toLocaleString('en-US', {
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit',
			hour12: true
		});
	}

	function formatEta(seconds: number): string {
		if (seconds < 60) return `约 ${seconds} 秒`;
		const m = Math.round(seconds / 60);
		if (m < 60) return `约 ${m} 分钟`;
		return `约 ${Math.floor(m / 60)} 小时 ${m % 60} 分钟`;
	}

	const anyTaskRunning = $derived(
		taskBoard?.videoTask.running ||
			taskBoard?.dynamicSources.some((s) => s.active) ||
			taskBoard?.scanTask.state !== 'idle'
	);

	async function loadTaskBoard() {
		try {
			taskBoard = (await api.getTaskBoard()).data;
		} catch {
			// 轮询失败静默，下轮重试
		}
	}

	async function loadDashboard() {
		loading = true;
		try {
			const response = await api.getDashboard();
			dashboardData = response.data;
		} catch (error) {
			console.error('加载仪表盘数据失败：', error);
			toast.error('加载仪表盘数据失败', {
				description: (error as ApiError).message
			});
		} finally {
			loading = false;
		}
	}

	async function handleTriggerDownload() {
		triggering = true;
		try {
			await api.triggerDownloadTask();
			toast.success('已触发下载任务', {
				description: '任务将立即开始执行'
			});
		} catch (error) {
			console.error('触发下载任务失败：', error);
			toast.error('触发下载任务失败', {
				description: (error as ApiError).message
			});
		} finally {
			triggering = false;
		}
	}

	const videoChartConfig = {
		videos: {
			label: '视频数量',
			color: 'var(--primary)'
		}
	} satisfies Chart.ChartConfig;

	const memoryChartConfig = {
		used: {
			label: '整体占用',
			color: 'var(--primary)'
		},
		process: {
			label: '程序占用',
			color: 'oklch(from var(--primary) calc(l * 0.6) c h)'
		}
	} satisfies Chart.ChartConfig;

	const cpuChartConfig = {
		used: {
			label: '整体占用',
			color: 'var(--primary)'
		},
		process: {
			label: '程序占用',
			color: 'oklch(from var(--primary) calc(l * 0.6) c h)'
		}
	} satisfies Chart.ChartConfig;

	function pushSysInfo(data: SysInfo) {
		memoryHistory = [
			...memoryHistory.slice(-14),
			{
				time: data.timestamp,
				used: data.used_memory,
				process: data.process_memory
			}
		];
		cpuHistory = [
			...cpuHistory.slice(-14),
			{
				time: data.timestamp,
				used: data.used_cpu,
				process: data.process_cpu
			}
		];
	}

	const diskUsagePercent = $derived(
		sysInfo ? ((sysInfo.total_disk - sysInfo.available_disk) / sysInfo.total_disk) * 100 : 0
	);

	onMount(() => {
		setBreadcrumb([{ label: '仪表盘' }]);

		unsubscribeSysInfo = api.subscribeToSysInfo((data) => {
			sysInfo = data;
			pushSysInfo(data);
		});
		unsubscribeTasks = api.subscribeToTasks((data: TaskStatus) => {
			taskStatus = data;
		});
		loadDashboard();
		loadTaskBoard();
		taskBoardTimer = setInterval(loadTaskBoard, 5000);
		return () => {
			if (taskBoardTimer) {
				clearInterval(taskBoardTimer);
				taskBoardTimer = null;
			}
			if (unsubscribeSysInfo) {
				unsubscribeSysInfo();
				unsubscribeSysInfo = null;
			}
			if (unsubscribeTasks) {
				unsubscribeTasks();
				unsubscribeTasks = null;
			}
		};
	});
</script>

<svelte:head>
	<title>仪表盘 - Bili Sync</title>
</svelte:head>

<div class="space-y-6">
	<Collapsible.Root bind:open={taskBoardOpen}>
		<Collapsible.Trigger
			class="hover:bg-accent/50 flex w-full items-center justify-between rounded-lg border px-4 py-3 text-sm font-medium transition-colors"
		>
			<span class="flex items-center gap-2">
				<RadarIcon class="text-primary h-4 w-4" />
				任务看板
				{#if anyTaskRunning}
					<span class="relative flex h-2 w-2">
						<span
							class="bg-emerald-400 absolute inline-flex h-full w-full animate-ping rounded-full opacity-75"
						></span>
						<span class="bg-emerald-500 relative inline-flex h-2 w-2 rounded-full"></span>
					</span>
				{/if}
			</span>
			<ChevronDownIcon
				class="text-muted-foreground h-4 w-4 transition-transform {taskBoardOpen
					? 'rotate-180'
					: ''}"
			/>
		</Collapsible.Trigger>
		<Collapsible.Content>
			<div class="grid gap-4 pt-3 md:grid-cols-3">
				<!-- 视频更新任务 -->
				<Card>
					<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
						<CardTitle class="text-sm font-medium">视频更新</CardTitle>
						<VideoIcon class="text-muted-foreground h-4 w-4" />
					</CardHeader>
					<CardContent>
						{#if taskBoard?.videoTask.running}
							<div class="flex items-center gap-2 text-sm">
								<Badge variant="default">进行中</Badge>
								<span class="min-w-0 truncate">
									{taskBoard.videoTask.phase}「{taskBoard.videoTask.currentTarget}」
								</span>
							</div>
							{#if taskBoard.videoTask.totalSources > 0}
								<Progress
									value={(taskBoard.videoTask.currentSourceIndex /
										taskBoard.videoTask.totalSources) *
										100}
									class="mt-2 h-1.5"
								/>
								<div class="text-muted-foreground mt-1 text-xs">
									第 {taskBoard.videoTask.currentSourceIndex} / {taskBoard.videoTask.totalSources} 个源
								</div>
							{/if}
						{:else}
							<div class="text-muted-foreground text-sm">
								正常运行中 · 等待下轮任务（周期 20 分钟）
							</div>
						{/if}
					</CardContent>
				</Card>

				<!-- 动态评论任务 -->
				<Card>
					<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
						<CardTitle class="text-sm font-medium">动态评论</CardTitle>
						<MessagesSquareIcon class="text-muted-foreground h-4 w-4" />
					</CardHeader>
					<CardContent>
						{#if (taskBoard?.dynamicSources ?? []).length === 0}
							<div class="text-muted-foreground text-sm">暂无动态源</div>
						{:else}
							<div class="space-y-2">
								{#each taskBoard?.dynamicSources ?? [] as source (source.id)}
									<div>
										<div class="flex items-center justify-between gap-2 text-sm">
											<span class="flex min-w-0 items-center gap-1.5">
												{#if source.active}
													<LoaderCircleIcon
														class="text-primary h-3.5 w-3.5 shrink-0 animate-spin"
													/>
												{:else}
													<CircleIcon class="text-muted-foreground h-3.5 w-3.5 shrink-0" />
												{/if}
												<span class="min-w-0 truncate">{source.name}</span>
											</span>
											{#if source.active}
												<span class="text-muted-foreground shrink-0 text-xs">
													{source.phase}
													{source.current}/{source.total}
												</span>
											{:else}
												<span class="text-muted-foreground shrink-0 text-xs">
													积压 {source.pending} 条
												</span>
											{/if}
										</div>
										{#if source.active && source.total > 0}
											<Progress value={(source.current / source.total) * 100} class="mt-1 h-1.5" />
										{/if}
										{#if source.etaSeconds !== null}
											<div class="text-muted-foreground mt-0.5 text-xs">
												{#if source.active}预计剩余{:else}预计耗时{/if}
												{formatEta(source.etaSeconds)}
											</div>
										{/if}
									</div>
								{/each}
							</div>
						{/if}
					</CardContent>
				</Card>

				<!-- 删除检测任务 -->
				<Card>
					<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
						<CardTitle class="text-sm font-medium">删除检测</CardTitle>
						<ScanSearchIcon class="text-muted-foreground h-4 w-4" />
					</CardHeader>
					<CardContent>
						{#if taskBoard?.scanTask.state === 'running'}
							<div class="flex items-center gap-2 text-sm">
								<Badge variant="default">检测中</Badge>
								<span class="text-muted-foreground text-xs">
									第 {taskBoard.scanTask.current} / {taskBoard.scanTask.total} 个源
								</span>
							</div>
							{#if taskBoard.scanTask.total > 0}
								<Progress
									value={(taskBoard.scanTask.current / taskBoard.scanTask.total) * 100}
									class="mt-2 h-1.5"
								/>
							{/if}
						{:else if taskBoard?.scanTask.state === 'queued'}
							<div class="flex items-center gap-2 text-sm">
								<Badge variant="secondary">排队中</Badge>
								<span class="text-muted-foreground text-xs">等待当前任务结束后执行</span>
							</div>
						{:else}
							<div class="text-muted-foreground text-sm">未进行</div>
						{/if}
					</CardContent>
				</Card>
			</div>
		</Collapsible.Content>
	</Collapsible.Root>

	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-muted-foreground">加载中...</div>
		</div>
	{:else}
		<div class="grid gap-4 md:grid-cols-3">
			<Card class="md:col-span-1">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">存储空间</CardTitle>
					<HardDriveIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent>
					{#if sysInfo}
						<div class="space-y-2">
							<div class="flex items-center justify-between">
								<div class="text-2xl font-bold">{formatBytes(sysInfo.available_disk)} 可用</div>
								<div class="text-muted-foreground text-sm">
									共 {formatBytes(sysInfo.total_disk)}
								</div>
							</div>
							<Progress value={diskUsagePercent} class="h-2" />
							<div class="text-muted-foreground text-xs">
								已使用 {diskUsagePercent.toFixed(1)}% 的存储空间
							</div>
						</div>
					{:else}
						<div class="text-muted-foreground text-sm">加载中...</div>
					{/if}
				</CardContent>
			</Card>
			<Card class="md:col-span-2">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">当前监听</CardTitle>
					<DatabaseIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent>
					{#if dashboardData}
						<div class="grid grid-cols-2 gap-4">
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-2">
									<HeartIcon class="text-muted-foreground h-4 w-4" />
									<span class="text-sm">收藏夹</span>
								</div>
								<Badge variant="outline">{dashboardData.enabled_favorites}</Badge>
							</div>
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-2">
									<FolderIcon class="text-muted-foreground h-4 w-4" />
									<span class="text-sm">合集 / 列表</span>
								</div>
								<Badge variant="outline">{dashboardData.enabled_collections}</Badge>
							</div>
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-2">
									<UserIcon class="text-muted-foreground h-4 w-4" />
									<span class="text-sm">投稿</span>
								</div>
								<Badge variant="outline">{dashboardData.enabled_submissions}</Badge>
							</div>
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-2">
									<ClockIcon class="text-muted-foreground h-4 w-4" />
									<span class="text-sm">稍后再看</span>
								</div>
								<Badge variant="outline">
									{dashboardData.enable_watch_later ? '启用' : '禁用'}
								</Badge>
							</div>
						</div>
					{:else}
						<div class="text-muted-foreground text-sm">加载中...</div>
					{/if}
				</CardContent>
			</Card>
		</div>

		<div class="grid gap-4 md:grid-cols-3">
			<Card class="max-w-full overflow-hidden md:col-span-2">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">最近入库</CardTitle>
					<VideoIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent>
					{#if dashboardData && dashboardData.videos_by_day.length > 0}
						<div class="mb-4 space-y-2">
							<div class="flex items-center justify-between text-sm">
								<span>近七日新增视频</span>
								<span class="font-medium"
									>{dashboardData.videos_by_day.reduce((sum, v) => sum + v.cnt, 0)} 个</span
								>
							</div>
						</div>
						<Chart.Container config={videoChartConfig} class="h-[200px] w-full">
							<BarChart
								data={dashboardData.videos_by_day}
								x="day"
								axis="x"
								series={[
									{
										key: 'cnt',
										label: '新增视频',
										color: videoChartConfig.videos.color
									}
								]}
								props={{
									bars: {
										stroke: 'none',
										rounded: 'all',
										radius: 8,
										initialHeight: 0
									},
									highlight: { area: { fill: 'none' } },
									xAxis: { format: () => '' }
								}}
							>
								{#snippet tooltip()}
									<MyChartTooltip indicator="line" />
								{/snippet}
							</BarChart>
						</Chart.Container>
					{:else}
						<div class="text-muted-foreground flex h-[200px] items-center justify-center text-sm">
							暂无视频统计数据
						</div>
					{/if}</CardContent
				>
			</Card>
			<Card class="max-w-full md:col-span-1">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">下载任务状态</CardTitle>
					<CloudDownloadIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent>
					{#if taskStatus}
						<div class="space-y-4">
							<div class="grid grid-cols-1 gap-6">
								<div class="mb-4 space-y-2">
									<div class="flex items-center justify-between text-sm">
										<span>当前任务状态</span>
										<Badge variant={taskStatus.is_running ? 'default' : 'outline'}>
											{taskStatus.is_running ? '运行中' : '未运行'}
										</Badge>
									</div>
								</div>
								<div class="flex items-center justify-between">
									<div class="flex items-center gap-2">
										<PlayIcon class="text-muted-foreground h-4 w-4" />
										<span class="text-sm">开始运行</span>
									</div>
									<span class="text-muted-foreground text-sm">
										{taskStatus.last_run
											? new Date(taskStatus.last_run).toLocaleString('en-US', {
													month: '2-digit',
													day: '2-digit',
													hour: '2-digit',
													minute: '2-digit',
													second: '2-digit',
													hour12: true
												})
											: '-'}
									</span>
								</div>
								<div class="flex items-center justify-between">
									<div class="flex items-center gap-2">
										<CircleCheckBigIcon class="text-muted-foreground h-4 w-4" />
										<span class="text-sm">运行结束</span>
									</div>
									<span class="text-muted-foreground text-sm">
										{taskStatus.last_finish
											? new Date(taskStatus.last_finish).toLocaleString('en-US', {
													month: '2-digit',
													day: '2-digit',
													hour: '2-digit',
													minute: '2-digit',
													second: '2-digit',
													hour12: true
												})
											: '-'}
									</span>
								</div>
								<div class="flex items-center justify-between">
									<div class="flex items-center gap-2">
										<CalendarIcon class="text-muted-foreground h-4 w-4" />
										<span class="text-sm">下次运行</span>
									</div>
									<span class="text-muted-foreground text-sm">
										{taskStatus.next_run
											? new Date(taskStatus.next_run).toLocaleString('en-US', {
													month: '2-digit',
													day: '2-digit',
													hour: '2-digit',
													minute: '2-digit',
													second: '2-digit',
													hour12: true
												})
											: '-'}
									</span>
								</div>
							</div>
							<div class="mt-6 border-t pt-4">
								<Button
									class="w-full"
									size="sm"
									onclick={handleTriggerDownload}
									disabled={triggering || (taskStatus?.is_running ?? false)}
								>
									<DownloadIcon class="h-4 w-4" />
									{triggering
										? '触发中...'
										: taskStatus?.is_running
											? '任务运行中'
											: '立即执行下载任务'}
								</Button>
							</div>
						</div>
					{:else}
						<div class="text-muted-foreground text-sm">加载中...</div>
					{/if}
				</CardContent>
			</Card>
		</div>

		<!-- 第三行：系统监控 -->
		<div class="grid gap-4 md:grid-cols-2">
			<!-- 内存使用情况 -->
			<Card class="overflow-hidden">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">内存使用情况</CardTitle>
					<MemoryStickIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent>
					{#if sysInfo}
						<div class="mb-4 space-y-2">
							<div class="flex items-center justify-between text-sm">
								<span>当前内存使用</span>
								<span class="font-medium"
									>{formatBytes(sysInfo.used_memory)} / {formatBytes(sysInfo.total_memory)}</span
								>
							</div>
						</div>
					{/if}
					{#if memoryHistory.length > 0}
						<Chart.Container config={memoryChartConfig} class="h-[150px] w-full">
							<AreaChart
								data={memoryHistory}
								x="time"
								axis="x"
								series={[
									{
										key: 'used',
										label: memoryChartConfig.used.label,
										color: memoryChartConfig.used.color
									},
									{
										key: 'process',
										label: memoryChartConfig.process.label,
										color: memoryChartConfig.process.color
									}
								]}
								props={{
									area: {
										curve: curveNatural,
										line: { class: 'stroke-1' },
										'fill-opacity': 0.4
									},
									xAxis: {
										format: () => ''
									}
								}}
							>
								{#snippet tooltip()}
									<MyChartTooltip
										labelFormatter={(timestamp: number) => {
											return formatTimestamp(timestamp);
										}}
										valueFormatter={(v: number) => formatBytes(v)}
										indicator="line"
									/>
								{/snippet}
							</AreaChart>
						</Chart.Container>
					{:else}
						<div class="text-muted-foreground flex h-[200px] items-center justify-center text-sm">
							等待数据...
						</div>
					{/if}
				</CardContent>
			</Card>

			<Card class="overflow-hidden">
				<CardHeader class="flex flex-row items-center justify-between space-y-0 pb-2">
					<CardTitle class="text-sm font-medium">CPU 使用情况</CardTitle>
					<CpuIcon class="text-muted-foreground h-4 w-4" />
				</CardHeader>
				<CardContent class="overflow-hidden">
					{#if sysInfo}
						<div class="mb-4 space-y-2">
							<div class="flex items-center justify-between text-sm">
								<span>当前 CPU 使用率</span>
								<span class="font-medium">{formatCpu(sysInfo.used_cpu)}</span>
							</div>
						</div>
					{/if}
					{#if cpuHistory.length > 0}
						<Chart.Container config={cpuChartConfig} class="h-[150px] w-full">
							<AreaChart
								data={cpuHistory}
								x="time"
								axis="x"
								series={[
									{
										key: 'used',
										label: cpuChartConfig.used.label,
										color: cpuChartConfig.used.color
									},
									{
										key: 'process',
										label: cpuChartConfig.process.label,
										color: cpuChartConfig.process.color
									}
								]}
								props={{
									area: {
										curve: curveNatural,
										line: { class: 'stroke-1' },
										'fill-opacity': 0.4
									},
									xAxis: {
										format: () => ''
									}
								}}
							>
								{#snippet tooltip()}
									<MyChartTooltip
										labelFormatter={(timestamp: number) => {
											return formatTimestamp(timestamp);
										}}
										valueFormatter={(v: number) => formatCpu(v)}
										indicator="line"
									/>
								{/snippet}
							</AreaChart>
						</Chart.Container>
					{:else}
						<div class="text-muted-foreground flex h-[150px] items-center justify-center text-sm">
							等待数据...
						</div>
					{/if}
				</CardContent>
			</Card>
		</div>
	{/if}
</div>
