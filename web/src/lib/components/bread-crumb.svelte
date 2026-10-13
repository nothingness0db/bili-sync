<script lang="ts">
	import * as Breadcrumb from '$lib/components/ui/breadcrumb/index.js';

	export let items: Array<{
		href?: string;
		label: string;
	}> = [{ href: '/', label: '主页' }];
</script>

<Breadcrumb.Root>
	<Breadcrumb.List>
		{#each items as item, index (item.label)}
			<Breadcrumb.Item class="min-w-0">
				{#if item.href}
					<Breadcrumb.Link href={item.href} class="block max-w-[40vw] truncate" title={item.label}
						>{item.label}</Breadcrumb.Link
					>
				{:else}
					<Breadcrumb.Page class="block max-w-[40vw] truncate" title={item.label}
						>{item.label}</Breadcrumb.Page
					>
				{/if}
			</Breadcrumb.Item>
			{#if index < items.length - 1}
				<Breadcrumb.Separator class="min-w-0" />
			{/if}
		{/each}
	</Breadcrumb.List>
</Breadcrumb.Root>
