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
    const mice = state.mice.map((item, index) => ({
        ...item,
        type: 'mouse',
        device_id: item.device_id || `mouse:${item.product_id || 0}:${index}`,
    }));
    const keyboard = state.keyboard ? [{ ...state.keyboard, type: 'keyboard' }] : [];
    const bluetooth = state.bluetooth_devices.map(item => ({ ...item, type: 'bluetooth' }));
    return [...mice, ...keyboard, ...bluetooth];
}

// 使用安全 DOM 展示外部设备名、状态和读取时间，支持通过右上角编辑按钮行内重命名。
function card(device, t, onRename) {
    const online = device.online && device.percentage >= 0;
    const color = batteryColor(device);
    const root = node('div', { className: `device-card${online ? '' : ' offline'}` });
    root.setAttribute('data-device-id', device.device_id);
    const middle = node('div', { className: 'card-middle' });
    const name = node('div', { className: 'device-name', text: device.name });
    name.title = device.name;

    // 行内编辑设备名称
    function startEditing() {
        if (root.classList.contains('editing')) return;
        root.classList.add('editing');

        // syncNode 保留现有事件监听器，因此从当前 DOM 读取最新名称和身份。
        const originalName = name.textContent;
        const deviceId = root.getAttribute('data-device-id');
        const input = node('input', { className: 'device-name-input' });
        input.type = 'text';
        input.value = originalName;
        input.maxLength = 32;

        input.addEventListener('click', e => e.stopPropagation());
        input.addEventListener('mousedown', e => e.stopPropagation());

        let finished = false;
        const commit = (save) => {
            if (finished) return;
            finished = true;
            root.classList.remove('editing');
            const nextValue = input.value.trim();
            input.replaceWith(name);

            if (save && nextValue !== originalName && onRename) {
                onRename(deviceId, nextValue);
            }
        };

        input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                commit(true);
            } else if (e.key === 'Escape') {
                e.preventDefault();
                commit(false);
            }
        });

        input.addEventListener('blur', () => {
            commit(true);
        });

        name.replaceWith(input);
        input.focus();
        input.select();
    }

    const top = cardHeader(device, { online, color, t }, startEditing);

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

// 保留原卡片左侧品牌与状态组合、右侧编辑和移除按钮的层次和间距。
function cardHeader(device, options, onStartEdit) {
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

    const actions = node('div', { className: 'card-actions' });
    actions.append(editButton(device, options.t, onStartEdit));
    if (device.type !== 'mouse') actions.append(removeButton(device, options.t));
    top.append(actions);
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

// 构造卡片右上角的极简 X 图标
function createCloseSvg(size = 13) {
    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('width', String(size));
    svg.setAttribute('height', String(size));
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');

    const line1 = document.createElementNS(svgNS, 'line');
    line1.setAttribute('x1', '18');
    line1.setAttribute('y1', '6');
    line1.setAttribute('x2', '6');
    line1.setAttribute('y2', '18');

    const line2 = document.createElementNS(svgNS, 'line');
    line2.setAttribute('x1', '6');
    line2.setAttribute('y1', '6');
    line2.setAttribute('x2', '18');
    line2.setAttribute('y2', '18');

    svg.append(line1, line2);
    return svg;
}

// 构造卡片右上角的极简编辑铅笔图标
function createEditSvg(size = 13) {
    const svgNS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(svgNS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('width', String(size));
    svg.setAttribute('height', String(size));
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');

    const path = document.createElementNS(svgNS, 'path');
    path.setAttribute('d', 'M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z');
    svg.append(path);
    return svg;
}

// 构造右上角编辑设备名称按钮
function editButton(device, t, onStartEdit) {
    const titleText = t('device.rename_btn');
    const button = node('button', { className: 'card-btn-edit' });
    button.title = titleText;
    button.setAttribute('aria-label', titleText);
    button.append(createEditSvg(13));
    button.addEventListener('click', (e) => {
        e.stopPropagation();
        if (onStartEdit) onStartEdit();
    });
    return button;
}

// 身份通过 dataset 赋值，改用 X 图标呈现，同时通过 title 与 aria-label 保持悬停提示。
function removeButton(device, t) {
    const titleText = t(device.type === 'keyboard' ? 'device.unbind_btn' : 'device.remove_btn');
    const button = node('button', { className: 'card-btn-remove' });
    button.title = titleText;
    button.setAttribute('aria-label', titleText);
    button.append(createCloseSvg(13));
    Object.assign(button.dataset, { action: 'remove', type: device.type,
        id: device.device_id, name: device.name });
    return button;
}

// 每个页面持有自己的卡片缓存；原有结构、配色和动画全部沿用 card 构造器。
export function createDeviceRenderer(options = {}) {
    const { onRename } = options;
    let previous = new Map();
    return (state, t) => {
        const devices = cards(state);
        const next = new Map();
        const list = el('deviceList');
        devices.forEach((device, index) => {
            const key = deviceKey(device);
            const signature = JSON.stringify([device, state.config.effective_ui_language]);
            const entry = previous.get(key);

            // 若用户正在该卡片内行内编辑设备名，跳过 syncNode，避免被后台轮询打断输入
            if (entry?.element?.classList.contains('editing')) {
                if (list.children[index] !== entry.element) {
                    list.insertBefore(entry.element, list.children[index] || null);
                }
                next.set(key, entry);
                return;
            }

            const element = entry?.element || card(device, t, onRename);
            if (entry && entry.signature !== signature) syncNode(element, card(device, t, onRename));
            if (list.children[index] !== element) list.insertBefore(element, list.children[index] || null);
            next.set(key, { element, signature });
        });
        previous.forEach((entry, key) => { if (!next.has(key)) entry.element.remove(); });
        previous = next;
        el('emptyState').style.display = devices.length ? 'none' : 'flex';
        el('deviceStatus').textContent = state.read_state === 'ok' ? '' : state.read_error;
    };
}

// 设备换序时仍保留各自的节点和编辑状态；旧鼠标快照由 cards 补齐临时 ID。
function deviceKey(device) {
    return JSON.stringify([device.type, device.device_id]);
}
