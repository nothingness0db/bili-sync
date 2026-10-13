<script lang="ts">
	import { onDestroy } from 'svelte';
	import api from '$lib/api';
	import type { ApiError, DynamicDetailResponse, ReplyItem } from '$lib/types';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import AuthenticatedDynamicImage from '$lib/components/authenticated-dynamic-image.svelte';
	import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
	import UserIcon from '@lucide/svelte/icons/user';

	let {
		open = $bindable(false),
		sourceId,
		dynamicId,
		title = '动态详情',
		description = '查看动态正文、图片与本地保存的评论。'
	}: {
		open?: boolean;
		sourceId: number;
		dynamicId: string;
		title?: string;
		description?: string;
	} = $props();
	let loading = $state(false);
	let error = $state<string | null>(null);
	let detail = $state<DynamicDetailResponse | null>(null);
	let sequence = 0;

	async function load(source: number, id: string) {
		const current = ++sequence;
		loading = true;
		error = null;
		detail = null;
		try {
			const response = await api.getDynamicDetail(source, id);
			if (current === sequence) detail = response.data;
		} catch (cause) {
			if (current === sequence) error = (cause as ApiError).message;
		} finally {
			if (current === sequence) loading = false;
		}
	}

	$effect(() => {
		if (open) void load(sourceId, dynamicId);
		return () => {
			sequence += 1;
		};
	});
	onDestroy(() => {
		sequence += 1;
	});

	function statCount(stat: Record<string, unknown>, key: string): number {
		return (stat[key] as { count?: number } | undefined)?.count ?? 0;
	}
	function countReplies(replies: ReplyItem[]): number {
		return replies.reduce((count, reply) => count + 1 + countReplies(reply.subReplies), 0);
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="no-scrollbar max-h-[85vh] max-w-[90vw]! overflow-y-auto lg:max-w-[60vw]!">
		<Dialog.Title class="text-lg font-semibold">{title}</Dialog.Title>
		<Dialog.Description>{description}</Dialog.Description>
		{#if loading}
			<div class="flex items-center justify-center py-12">
				<div class="text-muted-foreground">加载中...</div>
			</div>
		{:else if error}
			<div role="alert" class="space-y-3 py-8 text-center">
				<p class="text-destructive text-sm">加载失败：{error}</p>
				<Button size="sm" variant="outline" onclick={() => load(sourceId, dynamicId)}>重试</Button>
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
							{#each detail.pics as _, i (detail.id + '-' + i)}
								<AuthenticatedDynamicImage
									{sourceId}
									dynamicId={detail.id}
									name={`pics/${String(i + 1).padStart(2, '0')}.jpg`}
									alt={`图片 ${i + 1}`}
									className="max-h-64 max-w-full rounded-lg border object-contain"
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
						<span class="text-sm font-medium"
							>本地评论（{countReplies(detail.replies)}，含楼中楼）</span
						>
					</div>
					{#if detail.replies.length > 0}
						<div class="space-y-4">
							{#each detail.replies as reply (reply.rpid)}
								{@render ReplyRow({ reply })}
							{/each}
						</div>
					{:else}
						<div class="text-muted-foreground py-6 text-center text-sm">
							暂无本地评论，等待评论同步后再查看。
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>

{#snippet ReplyRow({ reply, depth = 0 }: { reply: ReplyItem; depth?: number })}
	<div class="flex gap-3" style="margin-left: {depth > 0 ? 12 : 0}px">
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
				{#if !reply.valid}
					<Badge variant="secondary" class="text-[10px]">B 站已失效，本地保留</Badge>
				{/if}
				<span class="text-muted-foreground">
					{new Date(reply.ctime).toLocaleString('zh-CN')}
				</span>
				{#if reply.parentRpid}
					<Badge variant="secondary" class="text-[10px]">楼中楼</Badge>
				{/if}
			</div>
			<div class="text-muted-foreground mt-1 text-sm break-words whitespace-pre-wrap">
				{reply.content || '（无内容）'}
			</div>
			{#if reply.images.length > 0}
				<div class="mt-1 flex flex-wrap gap-2">
					{#each reply.images as _, i (detail!.id + '-' + reply.rpid + '-' + i)}
						<AuthenticatedDynamicImage
							{sourceId}
							dynamicId={detail!.id}
							name={`comments/${reply.rpid}_${i + 1}.jpg`}
							alt="评论图片"
							className="h-24 max-w-48 rounded-md border object-cover"
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
