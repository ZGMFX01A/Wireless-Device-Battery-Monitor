import { el, node, syncNode } from './dom.js';

// 原设计中各厂商品牌徽标使用固定配色类，外部名称不能成为任意 CSS 类。
const BRAND_CLASSES = Object.freeze({
    '罗技': 'logitech', logitech: 'logitech', '雷蛇': 'razer', razer: 'razer', rog: 'rog',
});
// 状态颜色与原设计的高电量、低电量和充电色保持一致。
const BATTERY_COLORS = Object.freeze({ offline: '#64748b', charging: '#06b6d4',
    normal: '#10b981', warning: '#f59e0b', low: '#ef4444' });
// 原电量条使用的高电量与提醒分界。
const NORMAL_BATTERY_LEVEL = 50;
const WARNING_BATTERY_LEVEL = 20;
const FULL_BATTERY_LEVEL = 100;

// 设备卡片颜色对应充电、高电量、提醒与危险状态。
function batteryColor(device) {
    if (!device.online || device.percentage < 0) return BATTERY_COLORS.offline;
    if (device.charging) return BATTERY_COLORS.charging;
    if (device.percentage >= NORMAL_BATTERY_LEVEL) return BATTERY_COLORS.normal;
    return device.percentage >= WARNING_BATTERY_LEVEL ? BATTERY_COLORS.warning : BATTERY_COLORS.low;
}

// 多种设备快照映射为统一卡片，蓝牙保留真实端点 ID。
function cards(state) {
    const mice = state.mice.map(item => ({ ...item, type: 'mouse' }));
    const keyboard = state.keyboard ? [{ ...state.keyboard, type: 'keyboard' }] : [];
    const bluetooth = state.bluetooth_devices.map(item => ({ ...item, type: 'bluetooth' }));
    return [...mice, ...keyboard, ...bluetooth];
}

// 使用安全 DOM 展示外部设备名、状态和读取时间，不解析设备数据为 HTML。
function card(device, t) {
    const online = device.online && device.percentage >= 0;
    const color = batteryColor(device);
    const root = node('div', { className: `device-card${online ? '' : ' offline'}` });
    const top = cardHeader(device, { online, color, t });
    const middle = node('div', { className: 'card-middle' });
    const name = node('div', { className: 'device-name', text: device.name });
    name.title = device.name;
    middle.append(name, batteryDisplay(device, { online, color, t }));
    const track = node('div', { className: 'battery-track' });
    const fill = node('div', { className: 'battery-fill' });
    fill.style.width = online ? `${Math.min(FULL_BATTERY_LEVEL, Math.max(0, device.percentage))}%` : '0%';
    fill.style.background = color;
    track.append(fill);
    const bottom = node('div', { className: 'card-bottom' });
    bottom.append(node('span', { text: t(`device.${device.type}`) }));
    const time = device.last_update > 0 ? t('device.updated_at', {
        time: new Date(device.last_update * 1000).toTimeString().split(' ')[0],
    }) : t('device.wait_update');
    bottom.append(node('span', { text: time }));
    root.append(top, middle, track, bottom);
    return root;
}

// 保留原卡片左侧品牌与状态组合、右侧移除按钮的层次和间距。
function cardHeader(device, options) {
    const top = node('div', { className: 'card-top' });
    const labels = node('div');
    labels.style.cssText = 'display:flex; align-items:center; gap:6px;';
    const brand = String(device.brand).toLowerCase();
    const brandClass = Object.hasOwn(BRAND_CLASSES, brand) ? BRAND_CLASSES[brand] : 'default';
    labels.append(node('span', { className: `brand-badge ${brandClass}`, text: device.brand }));
    const status = node('div', { className: 'status-tag' });
    const dot = node('span', { className: 'dot' });
    dot.style.background = options.color;
    status.append(dot, node('span', {
        text: device.status_text || options.t(options.online ? 'device.online' : 'device.offline'),
    }));
    labels.append(status);
    top.append(labels);
    if (device.type !== 'mouse') top.append(removeButton(device, options.t));
    return top;
}

// 恢复数字、百分号与充电徽标的独立节点，让原字号和对齐规则生效。
function batteryDisplay(device, options) {
    const display = node('div', { className: 'battery-display' });
    const value = node('span', { className: 'battery-number',
        text: options.online ? device.percentage : '--' });
    value.style.color = options.color;
    display.append(value, node('span', { className: 'battery-percent-sign',
        text: options.online ? '%' : '' }));
    if (device.charging) {
        display.append(node('span', { className: 'charging-badge', text: options.t('device.charging') }));
    }
    return display;
}

// 身份通过 dataset 赋值，不让引号或标签改变移除按钮的结构。
function removeButton(device, t) {
    const button = node('button', {
        className: 'card-btn-remove', text: t(device.type === 'keyboard' ? 'device.unbind_btn' : 'device.remove_btn'),
    });
    Object.assign(button.dataset, { action: 'remove', type: device.type,
        id: device.device_id, name: device.name });
    return button;
}

// 每个页面持有自己的卡片缓存；原有结构、配色和动画全部沿用 card 构造器。
export function createDeviceRenderer() {
    let previous = new Map();
    return (state, t) => {
        const devices = cards(state);
        const next = new Map();
        const list = el('deviceList');
        devices.forEach((device, index) => {
            const key = deviceKey(device, index);
            const signature = JSON.stringify([device, state.config.effective_ui_language]);
            const entry = previous.get(key);
            const element = entry?.element || card(device, t);
            if (entry && entry.signature !== signature) syncNode(element, card(device, t));
            if (list.children[index] !== element) list.insertBefore(element, list.children[index] || null);
            next.set(key, { element, signature });
        });
        previous.forEach((entry, key) => { if (!next.has(key)) entry.element.remove(); });
        previous = next;
        el('emptyState').style.display = devices.length ? 'none' : 'flex';
        el('deviceStatus').textContent = state.read_state === 'ok' ? '' : state.read_error;
    };
}

// 鼠标快照没有路径 ID，按产品号与位置复用显示节点；绑定设备使用真实设备 ID。
function deviceKey(device, index) {
    return JSON.stringify(device.type === 'mouse' ? [device.type, device.product_id, index]
        : [device.type, device.device_id]);
}
