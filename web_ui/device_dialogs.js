import { api, el, node, loadingNode } from './dom.js';

// 每个弹窗实例持有状态，异步回执不能写入后来打开的弹窗。
export class DeviceDialogs {
    constructor(options) {
        this.t = options.t;
        this.toast = options.toast;
        this.acceptState = options.acceptState;
        this.active = null;
        el('modalBtnCancel').addEventListener('click', () => this.close());
        el('modalBtnConfirm').addEventListener('click', () => {
            if (this.active?.phase === 'remove') this.confirmRemove(this.active);
            else this.confirm();
        });
    }

    // 打开设备对话框，外部标题和内容仅以文本或 DOM 节点呈现。
    open(kind, phase) {
        const context = { kind, phase, selected: null };
        this.active = context;
        el('modalOverlay').style.display = 'flex';
        el('modalTitle').textContent = this.t(`${kind}.dialog.title`);
        el('modalSubtitle').textContent = this.t(`${kind}.dialog.subtitle`);
        el('modalBtnConfirm').disabled = true;
        el('modalBtnConfirm').textContent = this.t(kind === 'keyboard' ? 'dialog.connect' : 'dialog.add');
        el('modalBody').replaceChildren(loadingNode(this.t(`${kind}.dialog.loading`)));
        return context;
    }

    // 关闭只隐藏弹窗；已经发给托盘的键盘绑定继续执行。
    close() {
        this.active = null;
        el('modalOverlay').style.display = 'none';
    }

    // 新增操作等待真实扫描回执，再展示这次请求对应的候选设备。
    async add(kind) {
        const context = this.open(kind, 'scan');
        try {
            const candidates = await api(`scan_${kind}_candidates`);
            if (this.active !== context) return;
            this.showCandidates(context, candidates);
        } catch (error) {
            // 硬件或主进程异常显示在当前弹窗，不能当作空候选列表。
            if (this.active === context) el('modalBody').replaceChildren(node('div', {
                className: 'modal-empty', text: this.t('toast.op_failed', { msg: error.message || error }),
            }));
        }
    }

    // 所有候选字段来自外设或 Windows 配对信息，只允许安全文本渲染。
    showCandidates(context, candidates) {
        const available = candidates.find(candidate => !candidate.is_bound);
        context.selected = available?.device_id;
        const list = node('div', { className: 'candidate-list' });
        candidates.forEach(candidate => {
            const item = node('div', { className: `candidate-item${candidate.is_bound ? ' disabled' : ''}` });
            item.dataset.deviceId = candidate.device_id;
            const details = node('div');
            details.append(node('div', { className: 'candidate-name',
                text: candidate.display_name || candidate.name || candidate.product_name }));
            details.append(node('div', { className: 'candidate-meta',
                text: candidate.product_name || this.t(candidate.is_bound ? 'device.bound' : 'device.ready') }));
            item.append(details);
            item.classList.toggle('selected', candidate.device_id === context.selected);
            item.addEventListener('click', () => this.select(context, candidate, item, list));
            list.append(item);
        });
        el('modalBody').replaceChildren(candidates.length ? list : node('div', {
            className: 'modal-empty', text: this.t(`${context.kind}.dialog.empty`),
        }));
        el('modalBtnConfirm').disabled = !context.selected;
        context.phase = 'select';
    }

    // 已添加候选不重复绑定，其余候选通过稳定设备 ID 单选。
    select(context, candidate, item, list) {
        if (candidate.is_bound || this.active !== context || context.phase !== 'select') return;
        context.selected = candidate.device_id;
        list.querySelectorAll('.candidate-item').forEach(child => child.classList.remove('selected'));
        item.classList.add('selected');
        el('modalBtnConfirm').disabled = false;
    }

    // 绑定等待命令完成，托盘正在连接期间显示进度且保留关闭弹窗入口。
    async confirm() {
        const context = this.active;
        if (!context?.selected || context.phase !== 'select') return;
        context.phase = 'binding';
        el('modalBtnConfirm').disabled = true;
        el('modalBody').replaceChildren(loadingNode(this.t('device.connecting')));
        try {
            const result = await api(`bind_${context.kind}`, context.selected);
            this.acceptState(result.state);
            if (this.active === context) this.close();
            this.toast(this.t('toast.bound_success'));
        } catch (error) {
            // 明确反馈失败，关闭后再次添加会重新扫描与提交，不复用旧回执。
            if (this.active === context) this.close();
            this.toast(this.t('toast.op_failed', { msg: error.message || error }));
        }
    }

    // 移除设备须等待托盘真正解除绑定，不能提前乐观隐藏设备卡片。
    remove(kind, device) {
        this.open(kind, 'remove');
        el('modalTitle').textContent = this.t(`${kind}.remove.title`);
        el('modalSubtitle').textContent = this.t(`${kind}.remove.message`);
        el('modalBody').textContent = device.name;
        el('modalBtnConfirm').textContent = this.t('dialog.remove');
        el('modalBtnConfirm').disabled = false;
        this.active.remove = device.id;
    }

    // 根据当前弹窗动作执行移除，ID 始终来自用户选择的那张卡片。
    async confirmRemove(context) {
        el('modalBtnConfirm').disabled = true;
        try {
            const result = context.kind === 'keyboard'
                ? await api('unbind_keyboard') : await api('unbind_bluetooth', context.remove);
            this.acceptState(result.state);
            if (this.active === context) this.close();
            this.toast(this.t('toast.removed_success'));
        } catch (error) {
            if (this.active === context) this.close();
            this.toast(this.t('toast.op_failed', { msg: error.message || error }));
        }
    }

    // 状态推送只更新正在绑定的弹窗，过期扫描状态不会覆盖候选列表。
    update(state) {
        if (this.active?.phase !== 'binding') return;
        const operation = state[`${this.active.kind}_operation`];
        if (operation.state === 'binding' && operation.message) {
            el('modalBody').querySelector('.modal-loading div:last-child').textContent = operation.message;
        }
    }

}
