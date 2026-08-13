<script lang="ts">
	import api from '$lib/api';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import { onMount, tick } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import PauseIcon from '@lucide/svelte/icons/pause';
	import PlayIcon from '@lucide/svelte/icons/play';
	import SearchIcon from '@lucide/svelte/icons/search';

	let unsubscribeLog: (() => void) | null = null;
	let logs: Array<{ timestamp: string; level: string; message: string }> = [];
	let shouldAutoScroll = true;
	let paused = $state(false);
	let main: HTMLElement | null = null;
	let scrollTimer: ReturnType<typeof setTimeout> | null = null;
	let filterText = $state('');
	let levelFilter = $state<'ALL' | 'INFO' | 'WARN' | 'ERROR'>('ALL');

	const filteredLogs = $derived(
		logs.filter((log) => {
			if (levelFilter !== 'ALL' && log.level !== levelFilter) return false;
			if (filterText && !log.message.includes(filterText)) return false;
			return true;
		})
	);

	function checkScrollPosition() {
		if (main) {
			const { scrollTop, scrollHeight, clientHeight } = main;
			shouldAutoScroll = scrollTop + clientHeight >= scrollHeight - 50;
		}
	}

	async function scrollToBottom() {
		await tick();
		if (shouldAutoScroll && !paused && main) {
			main.scrollTop = main.scrollHeight;
		}
	}

	onMount(() => {
		setBreadcrumb([{ label: '日志' }]);
		main = document.getElementById('main');
		main?.addEventListener('scroll', checkScrollPosition);
		unsubscribeLog = api.subscribeToLogs((data: string) => {
			logs = [...logs.slice(-799), JSON.parse(data)];
			if (scrollTimer) clearTimeout(scrollTimer);
			scrollTimer = setTimeout(scrollToBottom, 20);
		});
		return () => {
			if (scrollTimer) clearTimeout(scrollTimer);
			main?.removeEventListener('scroll', checkScrollPosition);
			if (unsubscribeLog) {
				unsubscribeLog();
				unsubscribeLog = null;
			}
		};
	});

	function getLevelColor(level: string) {
		switch (level) {
			case 'ERROR':
				return 'text-rose-600';
			case 'WARN':
				return 'text-yellow-600';
			case 'INFO':
			default:
				return 'text-emerald-600';
		}
	}
</script>

<svelte:head>
	<title>日志 - Bili Sync</title>
</svelte:head>

<div class="space-y-3">
	<div class="flex items-center gap-2">
		<div class="relative flex-1">
			<SearchIcon
				class="text-muted-foreground absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2"
			/>
			<Input
				class="pl-8"
				type="text"
				placeholder="过滤日志关键词（如源名、删除检测、动态）.."
				bind:value={filterText}
			/>
		</div>
		<div class="flex items-center gap-1">
			{#each ['ALL', 'INFO', 'WARN', 'ERROR'] as level (level)}
				<Button
					size="sm"
					variant={levelFilter === level ? 'default' : 'outline'}
					class="h-8 cursor-pointer"
					onclick={() => (levelFilter = level as 'ALL' | 'INFO' | 'WARN' | 'ERROR')}
				>
					{level}
				</Button>
			{/each}
		</div>
		<Button
			size="sm"
			variant="outline"
			class="h-8 cursor-pointer"
			onclick={() => {
				paused = !paused;
				if (!paused) {
					shouldAutoScroll = true;
					scrollToBottom();
				}
			}}
			title={paused ? '继续自动滚动' : '暂停自动滚动'}
		>
			{#if paused}
				<PlayIcon class="h-4 w-4" />
			{:else}
				<PauseIcon class="h-4 w-4" />
			{/if}
		</Button>
	</div>

	<div class="text-muted-foreground text-xs">
		共 {filteredLogs.length} 条{paused ? ' · 已暂停自动滚动' : ''}
	</div>

	<div class="space-y-1">
		{#each filteredLogs as log, index (index)}
			<div
				class="flex items-center gap-3 rounded-md p-1 font-mono text-xs {index % 2 === 0
					? 'bg-muted/50'
					: 'bg-background'}"
			>
				<span class="text-muted-foreground w-32 shrink-0">
					{log.timestamp}
				</span>
				<Badge
					class="w-16 shrink-0 justify-center {getLevelColor(
						log.level
					)} bg-primary/90 font-semibold"
				>
					{log.level}
				</Badge>
				<span class="flex-1 break-all">
					{log.message}
				</span>
			</div>
		{/each}
		{#if filteredLogs.length === 0}
			<div class="text-muted-foreground py-8 text-center">暂无匹配日志记录</div>
		{/if}
	</div>
</div>
