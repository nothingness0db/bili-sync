import { toast } from 'svelte-sonner';
import api from './api';
import type { SysInfo, TaskStatus } from './types';

// 支持的事件类型
export enum EventType {
	Logs = 'logs',
	Tasks = 'tasks',
	SysInfo = 'sysInfo'
}

// 服务器事件响应格式
interface ServerEvent {
	logs?: string;
	tasks?: TaskStatus;
	sysInfo?: SysInfo;
}

// 客户端事件请求格式
interface ClientEvent {
	subscribe?: EventType;
	unsubscribe?: EventType;
}

// 回调函数类型定义
type LogsCallback = (data: string) => void;
type TasksCallback = (data: TaskStatus) => void;
type SysInfoCallback = (data: SysInfo) => void;

export class WebSocketManager {
	private static instance: WebSocketManager;
	private socket: WebSocket | null = null;
	private connected = false;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private reconnectAttempts = 0;
	private maxReconnectAttempts = 5;
	private baseReconnectDelay = 1000;

	private logsSubscribers: Set<LogsCallback> = new Set();
	private tasksSubscribers: Set<TasksCallback> = new Set();
	private sysInfoSubscribers: Set<SysInfoCallback> = new Set();

	private subscribedEvents: Set<EventType> = new Set();
	private connectionPromise: Promise<void> | null = null;
	private rejectConnection: ((reason: Error) => void) | null = null;
	private shouldReconnect = false;

	private constructor() {}

	public static getInstance(): WebSocketManager {
		if (!WebSocketManager.instance) {
			WebSocketManager.instance = new WebSocketManager();
		}
		return WebSocketManager.instance;
	}

	// 连接 WebSocket
	public connect(): Promise<void> {
		if (this.connected) return Promise.resolve();
		if (this.connectionPromise) return this.connectionPromise;

		this.shouldReconnect = true;
		if (this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}

		let socket: WebSocket;
		try {
			const token = api.getAuthToken() || '';
			const protocol = window.location.protocol === 'https:' ? 'wss://' : 'ws://';
			// 使用 base64URL no padding 编码 token 以避免特殊字符问题
			socket = new WebSocket(
				`${protocol}${window.location.host}/api/ws`,
				btoa(token).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
			);
		} catch (error) {
			console.error('Failed to create WebSocket:', error);
			toast.error('创建 WebSocket 连接失败，请检查网络或稍后重试');
			this.scheduleReconnect();
			return Promise.reject(error);
		}

		this.socket = socket;
		this.connectionPromise = new Promise((resolve, reject) => {
			this.rejectConnection = reject;
			socket.onopen = () => {
				if (this.socket !== socket) return;
				this.connected = true;
				this.reconnectAttempts = 0;
				this.connectionPromise = null;
				this.rejectConnection = null;
				this.resubscribeEvents();
				resolve();
			};
			socket.onmessage = (event) => {
				if (this.socket === socket) this.handleMessage(event);
			};
			socket.onclose = () => {
				if (this.socket !== socket) return;
				this.socket = null;
				this.connected = false;
				this.connectionPromise = null;
				this.rejectConnection = null;
				reject(new Error('WebSocket 连接已关闭'));
				this.scheduleReconnect();
			};
			socket.onerror = (error) => {
				if (this.socket !== socket) return;
				console.error('WebSocket error:', error);
				reject(new Error('WebSocket 连接失败'));
				toast.error('WebSocket 连接发生错误，请检查网络或稍后重试');
			};
		});
		return this.connectionPromise;
	}

	private handleMessage(event: MessageEvent): void {
		try {
			const data = JSON.parse(event.data) as ServerEvent;

			if (data.logs !== undefined) {
				this.notifyLogsSubscribers(data.logs);
			} else if (data.tasks !== undefined) {
				this.notifyTasksSubscribers(data.tasks);
			} else if (data.sysInfo !== undefined) {
				this.notifySysInfoSubscribers(data.sysInfo);
			}
		} catch (error) {
			console.error('Failed to parse WebSocket message:', error, event.data);
			toast.error('解析 WebSocket 消息失败', {
				description: `消息内容：${event.data}\n错误信息：${error instanceof Error ? error.message : String(error)}`
			});
		}
	}

	private sendMessage(message: ClientEvent): void {
		if (!this.connected || !this.socket) return;

		try {
			this.socket.send(JSON.stringify(message));
		} catch (error) {
			console.error('Failed to send message:', error);
			toast.error('发送 WebSocket 消息失败', {
				description: `消息内容：${JSON.stringify(message)}\n错误信息：${error instanceof Error ? error.message : String(error)}`
			});
		}
	}

	private subscribe(eventType: EventType): void {
		if (this.subscribedEvents.has(eventType)) return;

		// 先保存订阅意图，首次连接失败后仍可在重连时恢复。
		this.subscribedEvents.add(eventType);
		if (this.connected) {
			this.sendMessage({ subscribe: eventType });
		} else {
			void this.connect().catch(() => {
				// 连接错误已提示，保留订阅供自动重连恢复。
			});
		}
	}

	// 取消订阅事件
	private unsubscribe(eventType: EventType): void {
		if (!this.subscribedEvents.delete(eventType)) return;
		// 断线时无需为取消订阅重新建立连接。
		this.sendMessage({ unsubscribe: eventType });
		if (this.subscribedEvents.size === 0 && this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
	}

	private resubscribeEvents(): void {
		for (const eventType of this.subscribedEvents) {
			this.sendMessage({ subscribe: eventType });
		}
	}

	private scheduleReconnect(): void {
		if (!this.shouldReconnect || this.subscribedEvents.size === 0) return;
		if (this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
		}

		if (this.reconnectAttempts >= this.maxReconnectAttempts) {
			console.log('Max reconnect attempts reached');
			return;
		}

		const delay = this.baseReconnectDelay * Math.pow(2, this.reconnectAttempts);
		console.log(`Scheduling reconnect in ${delay}ms`);

		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			if (!this.shouldReconnect || this.subscribedEvents.size === 0) return;
			this.reconnectAttempts++;
			void this.connect().catch(() => {
				// 自动重连失败由连接处理器安排下一次尝试。
			});
		}, delay);
	}

	public subscribeToLogs(callback: LogsCallback): () => void {
		this.logsSubscribers.add(callback);

		if (this.logsSubscribers.size === 1) {
			this.subscribe(EventType.Logs);
		}

		return () => {
			this.logsSubscribers.delete(callback);
			if (this.logsSubscribers.size === 0) {
				this.unsubscribe(EventType.Logs);
			}
		};
	}

	// 订阅任务状态
	public subscribeToTasks(callback: TasksCallback): () => void {
		this.tasksSubscribers.add(callback);

		if (this.tasksSubscribers.size === 1) {
			this.subscribe(EventType.Tasks);
		}

		return () => {
			this.tasksSubscribers.delete(callback);
			if (this.tasksSubscribers.size === 0) {
				this.unsubscribe(EventType.Tasks);
			}
		};
	}

	public subscribeToSysInfo(callback: SysInfoCallback): () => void {
		this.sysInfoSubscribers.add(callback);

		if (this.sysInfoSubscribers.size === 1) {
			this.subscribe(EventType.SysInfo);
		}

		return () => {
			this.sysInfoSubscribers.delete(callback);
			if (this.sysInfoSubscribers.size === 0) {
				this.unsubscribe(EventType.SysInfo);
			}
		};
	}

	private notifyLogsSubscribers(data: string): void {
		this.logsSubscribers.forEach((callback) => {
			try {
				callback(data);
			} catch (error) {
				console.error('Error in logs subscriber callback:', error);
			}
		});
	}

	private notifyTasksSubscribers(data: TaskStatus): void {
		this.tasksSubscribers.forEach((callback) => {
			try {
				callback(data);
			} catch (error) {
				console.error('Error in tasks subscriber callback:', error);
			}
		});
	}

	private notifySysInfoSubscribers(data: SysInfo): void {
		this.sysInfoSubscribers.forEach((callback) => {
			try {
				callback(data);
			} catch (error) {
				console.error('Error in sysInfo subscriber callback:', error);
			}
		});
	}

	public disconnect(): void {
		this.shouldReconnect = false;
		const socket = this.socket;
		this.socket = null;
		this.rejectConnection?.(new Error('WebSocket 已主动断开'));
		this.rejectConnection = null;

		if (this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}

		this.connected = false;
		this.connectionPromise = null;
		this.reconnectAttempts = 0;
		this.subscribedEvents.clear();
		socket?.close();
	}
}

export const wsManager = WebSocketManager.getInstance();
