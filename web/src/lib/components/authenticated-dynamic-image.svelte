<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import api from '$lib/api';

	export let sourceId: number;
	export let dynamicId: string;
	export let name: string;
	export let alt = '';
	export let className = '';

	let src = '';
	let objectUrl = '';
	let controller: AbortController | undefined;

	onMount(() => {
		controller = new AbortController();
		const token = api.getAuthToken();
		const headers: HeadersInit = token ? { Authorization: token } : {};
		const params = new URLSearchParams({ name });

		fetch(`/api/dynamic-sources/${sourceId}/dynamics/${dynamicId}/file?${params}`, {
			headers,
			signal: controller.signal
		})
			.then((response) => {
				if (!response.ok) throw new Error(`图片加载失败: ${response.status}`);
				return response.blob();
			})
			.then((blob) => {
				objectUrl = URL.createObjectURL(blob);
				src = objectUrl;
			})
			.catch(() => {
				// 图片不存在时保持空白，不影响动态详情的其余内容。
			});

		return () => controller?.abort();
	});

	onDestroy(() => {
		controller?.abort();
		if (objectUrl) URL.revokeObjectURL(objectUrl);
	});
</script>

{#if src}
	<img {src} {alt} class={className} />
{/if}
