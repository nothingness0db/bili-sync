<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import CircleCheckBigIcon from '@lucide/svelte/icons/circle-check-big';
	import CircleXIcon from '@lucide/svelte/icons/circle-x';
	import FolderIcon from '@lucide/svelte/icons/folder';
	import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import ChartLineIcon from '@lucide/svelte/icons/chart-line';
	import SquarePenIcon from '@lucide/svelte/icons/square-pen';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import UserIcon from '@lucide/svelte/icons/user';
	import { toast } from 'svelte-sonner';
	import { goto } from '$app/navigation';
	import { setBreadcrumb } from '$lib/stores/breadcrumb';
	import type { ApiError, DynamicSourceDetail } from '$lib/types';
	import api from '$lib/api';

	let dynamicSources: DynamicSourceDetail[] = [];
	let loading = false;

	// 添加对话框
	let showAddDialog = false;
	let addForm = { upper_id: '', path: '', sync_reply: true };
	let adding = false;

	// 编辑对话框
	let showEditDialog = false;
	let editingSource: DynamicSourceDetail | null = null;
	let editingIdx = 0;
	let editForm = { path: '', enabled: false, sync_reply: false };
	let saving = false;

	// 删除对话框
	let showRemoveDialog = false;
	let removeSource: DynamicSourceDetail | null = null;
	let removing = false;

	async function loadDynamicSources() {
		loading = true;
		try {
			const response = await api.getDynamicSources();
			dynamicSources = response.data;
		} catch (error) {
			toast.error('加载动态源失败', {
				description: (error as ApiError).message
			});
		} finally {
			loading = false;
		}
	}

	function openAddDialog() {
		addForm = { upper_id: '', path: '', sync_reply: true };
		showAddDialog = true;
	}

	// 从空间页 URL 中解析 mid，支持粘贴整条链接
	function handleUpperInput(value: string) {
		const urlMatch = value.match(/space\.bilibili\.com\/(\d+)/) || value.match(/bilibili\.com\/opus\/(\d+)/);
		if (urlMatch) {
			// opus 链接里的数字是动态 id 不是 mid，仅 space 链接可用
			if (value.includes('space.bilibili.com')) {
				addForm.upper_id = urlMatch[1];
				return;
			}
		}
		// 纯数字直接保留
		if (/^\d+$/.test(value.trim())) {
			addForm.upper_id = value.trim();
		} else {
			addForm.upper_id = '';
		}
	}

	async function handleAdd() {
		if (!addForm.upper_id || !addForm.path.trim()) {
			toast.error('请填写完整的动态源信息');
			return;
		}
		adding = true;
		try {
			await api.insertDynamicSource({
				upper_id: parseInt(addForm.upper_id),
				path: addForm.path,
				sync_reply: addForm.sync_reply
			});
			toast.success('添加成功');
			showAddDialog = false;
			loadDynamicSources();
		} catch (error) {
			toast.error('添加失败', {
				description: (error as ApiError).message
			});
		} finally {
			adding = false;
		}
	}

	function openEditDialog(source: DynamicSourceDetail, idx: number) {
		editingSource = source;
		editingIdx = idx;
		editForm = {
			path: source.path,
			enabled: source.enabled,
			sync_reply: source.syncReply
		};
		showEditDialog = true;
	}

	async function saveEdit() {
		if (!editingSource) return;
		if (!editForm.path?.trim()) {
			toast.error('路径不能为空');
			return;
		}
		saving = true;
		try {
			await api.updateDynamicSource(editingSource.id, {
				path: editForm.path,
				enabled: editForm.enabled,
				syncReply: editForm.sync_reply
			});
			dynamicSources[editingIdx] = {
				...dynamicSources[editingIdx],
				path: editForm.path,
				enabled: editForm.enabled,
				syncReply: editForm.sync_reply
			};
			dynamicSources = [...dynamicSources];
			showEditDialog = false;
			toast.success('保存成功');
		} catch (error) {
			toast.error('保存失败', {
				description: (error as ApiError).message
			});
		} finally {
			saving = false;
		}
	}

	function openRemoveDialog(source: DynamicSourceDetail, idx: number) {
		removeSource = source;
		showRemoveDialog = true;
	}

	async function removeDynamicSource() {
		if (!removeSource) return;
		removing = true;
		try {
			await api.removeDynamicSource(removeSource.id);
			dynamicSources = dynamicSources.filter((s) => s.id !== removeSource!.id);
			showRemoveDialog = false;
			toast.success('删除动态源成功');
		} catch (error) {
			toast.error('删除动态源失败', {
				description: (error as ApiError).message
			});
		} finally {
			removing = false;
		}
	}

	onMount(() => {
		setBreadcrumb([{ label: '动态源' }]);
		loadDynamicSources();
	});
</script>

<svelte:head>
	<title>动态源管理 - Bili Sync</title>
</svelte:head>

<div class="space-y-6">
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-muted-foreground">加载中...</div>
		</div>
	{:else}
		<div class="mb-4 flex items-center justify-between">
			<div></div>
			<Button size="sm" onclick={openAddDialog} class="flex items-center gap-2">
				<PlusIcon class="h-4 w-4" />
				添加动态源
			</Button>
		</div>
		{#if dynamicSources.length > 0}
			<div class="overflow-x-auto">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="w-[20%]">UP 主</Table.Head>
							<Table.Head class="w-[28%]">保存路径</Table.Head>
							<Table.Head class="w-[12%]">动态数</Table.Head>
							<Table.Head class="w-[12%]">评论数</Table.Head>
							<Table.Head class="w-[10%]">同步评论</Table.Head>
							<Table.Head class="w-[10%]">启用状态</Table.Head>
							<Table.Head class="w-[8%] text-right">操作</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each dynamicSources as source, index (source.id)}
							<Table.Row>
								<Table.Cell class="font-medium">
									<div class="flex items-center gap-2">
										<UserIcon class="text-muted-foreground h-4 w-4" />
										{source.upperName}
										<span class="text-muted-foreground text-xs">({source.upperId})</span>
									</div>
								</Table.Cell>
								<Table.Cell>
									<div
										class="bg-secondary hover:bg-secondary/80 flex w-fit cursor-text items-center gap-2 rounded-md px-2.5 py-1.5 transition-colors"
									>
										<FolderIcon class="text-foreground/70 h-3.5 w-3.5 shrink-0" />
										<span class="text-foreground/70 font-mono text-xs font-medium select-text">
											{source.path || '未设置'}
										</span>
									</div>
								</Table.Cell>
								<Table.Cell>
									<Badge variant="secondary" class="flex w-fit items-center gap-1.5">
										{source.dynamicCount}
									</Badge>
								</Table.Cell>
								<Table.Cell>
									<Badge variant="secondary" class="flex w-fit items-center gap-1.5">
										{source.replyCount}
									</Badge>
								</Table.Cell>
								<Table.Cell>
									{#if source.syncReply}
										<Badge
											class="flex w-fit items-center gap-1.5 bg-emerald-700 text-emerald-100"
										>
											<MessagesSquareIcon class="h-3 w-3" />
											开启
										</Badge>
									{:else}
										<Badge variant="secondary" class="flex w-fit items-center gap-1.5">
											关闭
										</Badge>
									{/if}
								</Table.Cell>
								<Table.Cell>
									{#if source.enabled}
										<Badge class="flex w-fit items-center gap-1.5 bg-emerald-700 text-emerald-100">
											<CircleCheckBigIcon class="h-3 w-3" />
											已启用
										</Badge>
									{:else}
										<Badge class="flex w-fit items-center gap-1.5 bg-rose-700 text-rose-100">
											<CircleXIcon class="h-3 w-3" />
											已禁用
										</Badge>
									{/if}
								</Table.Cell>
								<Table.Cell class="text-right">
									<Tooltip.Root disableHoverableContent={true}>
										<Tooltip.Trigger>
											<Button
												size="sm"
												variant="outline"
												onclick={() => goto(`/dynamic-sources/${source.id}`)}
												class="h-8 w-8 p-0"
											>
												<ChartLineIcon class="h-3 w-3" />
											</Button>
										</Tooltip.Trigger>
										<Tooltip.Content>
											<p class="text-xs">数据图表</p>
										</Tooltip.Content>
									</Tooltip.Root>
									<Tooltip.Root disableHoverableContent={true}>
										<Tooltip.Trigger>
											<Button
												size="sm"
												variant="outline"
												onclick={() => openEditDialog(source, index)}
												class="h-8 w-8 p-0"
											>
												<SquarePenIcon class="h-3 w-3" />
											</Button>
										</Tooltip.Trigger>
										<Tooltip.Content>
											<p class="text-xs">编辑</p>
										</Tooltip.Content>
									</Tooltip.Root>
									<Tooltip.Root disableHoverableContent={true}>
										<Tooltip.Trigger>
											<Button
												size="sm"
												variant="outline"
												onclick={() => openRemoveDialog(source, index)}
												class="h-8 w-8 p-0"
											>
												<Trash2Icon class="h-3 w-3" />
											</Button>
										</Tooltip.Trigger>
										<Tooltip.Content>
											<p class="text-xs">删除</p>
										</Tooltip.Content>
									</Tooltip.Root>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{:else}
			<div class="flex flex-col items-center justify-center py-12">
				<MessagesSquareIcon class="text-muted-foreground mb-4 h-12 w-12" />
				<div class="text-muted-foreground mb-2 text-lg font-medium">暂无动态源</div>
				<p class="text-muted-foreground mb-4 text-center text-sm">
					还没有添加任何 UP 主动态订阅
				</p>
				<Button onclick={openAddDialog} class="flex items-center gap-2">
					<PlusIcon class="h-4 w-4" />
					添加动态源
				</Button>
			</div>
		{/if}
	{/if}

	<!-- 添加对话框 -->
	<Dialog.Root bind:open={showAddDialog}>
		<Dialog.Content>
			<Dialog.Title class="text-lg font-semibold">添加动态源</Dialog.Title>
			<div class="mt-4 space-y-4">
				<div>
					<Label for="upper_id" class="text-sm font-medium">UP 主 ID (mid)</Label>
					<Input
						id="upper_id"
						type="text"
						value={addForm.upper_id}
						oninput={(e) => handleUpperInput(e.currentTarget.value)}
						placeholder="输入 UP 主 ID，或粘贴空间页链接（space.bilibili.com/xxx）"
						class="mt-1"
					/>
					<p class="text-muted-foreground mt-1 text-xs">
						支持粘贴完整链接自动解析，如 https://space.bilibili.com/495335682/dynamic
					</p>
				</div>
				<div>
					<Label for="path" class="text-sm font-medium">保存路径</Label>
					<Input
						id="path"
						type="text"
						bind:value={addForm.path}
						placeholder="请输入保存路径，例如：/path/to/download"
						class="mt-1"
					/>
				</div>
				<div class="flex items-center space-x-2">
					<Switch bind:checked={addForm.sync_reply} />
					<Label class="text-sm font-medium">同步该 UP 主动态下的评论</Label>
				</div>
				<p class="text-muted-foreground text-xs">
					动态同步包含正文、图片、点赞/评论/转发数、原始 JSON；开启评论后还会拉取全部评论及楼中楼回复。
				</p>
			</div>
			<div class="mt-6 flex justify-end gap-2">
				<Button variant="outline" onclick={() => (showAddDialog = false)} disabled={adding} class="px-4">
					取消
				</Button>
				<Button onclick={handleAdd} disabled={adding} class="px-4">
					{adding ? '添加中...' : '添加'}
				</Button>
			</div>
		</Dialog.Content>
	</Dialog.Root>

	<!-- 编辑对话框 -->
	<Dialog.Root bind:open={showEditDialog}>
		<Dialog.Content>
			<Dialog.Title class="text-lg font-semibold">编辑动态源: {editingSource?.upperName || ''}</Dialog.Title>
			<div class="mt-6 space-y-6">
				<div>
					<Label for="edit-path" class="text-sm font-medium">保存路径</Label>
					<Input
						id="edit-path"
						type="text"
						bind:value={editForm.path}
						placeholder="请输入保存路径"
						class="mt-2"
					/>
				</div>
				<div class="flex items-center space-x-2">
					<Switch bind:checked={editForm.enabled} />
					<Label class="text-sm font-medium">启用此动态源</Label>
				</div>
				<div class="flex items-center space-x-2">
					<Switch bind:checked={editForm.sync_reply} />
					<Label class="text-sm font-medium">同步评论</Label>
				</div>
			</div>
			<div class="mt-8 flex justify-end gap-3">
				<Button variant="outline" onclick={() => (showEditDialog = false)} disabled={saving}>
					取消
				</Button>
				<Button onclick={saveEdit} disabled={saving}>
					{saving ? '保存中...' : '保存'}
				</Button>
			</div>
		</Dialog.Content>
	</Dialog.Root>

	<!-- 删除对话框 -->
	<AlertDialog.Root bind:open={showRemoveDialog}>
		<AlertDialog.Content>
			<AlertDialog.Header>
				<AlertDialog.Title>删除动态源</AlertDialog.Title>
				<AlertDialog.Description>
					确定要删除动态源 <strong>"{removeSource?.upperName}"</strong> 吗？<br />
					删除后该动态源相关的所有动态与评论将从数据库中移除（不影响磁盘文件），该操作
					<span class="text-destructive font-medium">无法撤销</span>。<br />
				</AlertDialog.Description>
			</AlertDialog.Header>
			<AlertDialog.Footer>
				<AlertDialog.Cancel
					disabled={removing}
					onclick={() => {
						showRemoveDialog = false;
					}}>取消</AlertDialog.Cancel
				>
				<AlertDialog.Action onclick={removeDynamicSource} disabled={removing}>
					{removing ? '删除中' : '删除'}
				</AlertDialog.Action>
			</AlertDialog.Footer>
		</AlertDialog.Content>
	</AlertDialog.Root>
</div>
