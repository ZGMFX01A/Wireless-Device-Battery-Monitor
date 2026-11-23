# 设备适配申请与信息采集

[English](#english) | [Русский](#русский)

申请新设备适配，或反馈已有设备的识别、电量、充电及连接状态问题，请选择对应语言的表单。三份表单的字段、必填要求和采集命令一致。

| 语言 / Language / Язык | 表单 / Form / Форма |
| --- | --- |
| 简体中文 | [设备适配](https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/issues/new?template=device-support.zh-CN.yml) |
| English | [Device support](https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/issues/new?template=device-support.en.yml) |
| Русский | [Поддержка устройства](https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/issues/new?template=device-support.ru.yml) |

## 提交哪些信息

- **品牌与完整型号**：包含 Pro / Max / SE、代数、硬件版本等后缀。不要只写品牌、系列名或简称。
- **官方驱动下载地址或官方网页驱动地址**：至少提供一个与该型号对应的官方地址；如需在页面中选择型号、区域或版本，请注明选项。
- **连接方式及接收器**：明确 2.4G、蓝牙或 USB 数据线；接收器请注明型号、原装/替换/共享/高回报率版本、工作模式和配对设备。
- **版本信息**：本软件版本、Windows 版本与内部版本号（`winver`）、设备及接收器固件、官方驱动版本。查不到的版本请写“未知”。
- **问题与官方驱动对照**：复现步骤、实际/预期结果，官方驱动能否识别设备、显示电量和充电状态。尽量提供同一时间的两边读数，并标注官方驱动运行、退出或未测试。
- **下面的设备识别信息**，以及已测试的休眠唤醒、充电和连接方式切换结果。

## 通用设备识别命令

连接目标设备并将其唤醒，在 Windows PowerShell 中完整运行以下命令。命令无需预先知道 VID/PID，只读取 Windows 中当前存在的 USB、HID 和常见蓝牙设备节点，不会向外设发送电量查询或配置命令。

```powershell
Get-CimInstance -ClassName Win32_PnPEntity -Filter 'Present = True' -ErrorAction Stop |
  Where-Object { $_.PNPDeviceID -match '^(USB|HID|BTHENUM|BTHLEDEVICE|BTHLE)\\' } |
  Sort-Object PNPClass, Name, PNPDeviceID |
  Format-List Name, Manufacturer, PNPClass, Status, ConfigManagerErrorCode, PNPDeviceID,
    @{Name='HardwareID'; Expression={$_.HardwareID -join "`n"}},
    @{Name='CompatibleID'; Expression={$_.CompatibleID -join "`n"}}
```

使用 `Win32_PnPEntity` 是为了直接取得真实的 `HardwareID` 与 `CompatibleID` 数组；只在 `Get-PnpDevice` 后追加 `Format-List HardwareID` 不保证已取回该属性。使用换行展开数组，避免多条硬件标识被显示成省略号。保留状态异常的当前设备，便于定位驱动或枚举问题，不只筛选 `Status OK`。

字段含义：

| 字段 | 用途 |
| --- | --- |
| `Name` / `Manufacturer` / `PNPClass` | Windows 识别的名称、厂商与设备类别；名称可能是通用名称，不能代替完整产品型号。 |
| `Status` / `ConfigManagerErrorCode` | Windows 设备状态与设备管理器错误码，帮助确认是否存在驱动或枚举异常。 |
| `PNPDeviceID` | 设备实例路径，帮助区分 USB 接收器、复合接口和 HID collection。 |
| `HardwareID` | 硬件匹配标识，通常可提供 VID/PID、REV、MI、COL 等线索，具体字段取决于设备。 |
| `CompatibleID` | 兼容匹配标识；部分 HID 条目包含 Usage Page / Usage（UP/U）线索，不等于完整 HID 报告描述符。 |

这些属性的定义见 [Microsoft：Win32_PnPEntity](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-pnpentity)。

## 怎样确认哪些条目属于目标设备

1. **USB / 2.4G 接收器**：插入目标接收器或数据线时运行一次，拔出后再运行一次，找出新增或消失的条目。比较时请标明两次的连接状态；如有多个同型号接收器，请一次只改变目标接收器。
2. **保留全部相关接口**：同一个设备可能同时有 `USB\VID_...` 和多条 `HID\VID_...` 记录；不要只贴第一条，也不要丢掉不同 `MI_XX` / `COLXX` 的条目。仅关闭无线外设，通常不会让仍插着的接收器条目消失。
3. **蓝牙**：先在 Windows 中配对、连接并唤醒设备，再运行命令。配对节点可能在断开后保留；请同时注明 Windows 蓝牙设置中的连接情况，以及是否能显示电量，不能仅以 PnP 条目存在推断在线。
4. **提交原文**：将相关条目的文本贴入表单；若命令报错或没有结果，请保留错误原文，并补充“设备管理器 → 目标设备属性 → 详细信息 → 硬件 ID”的内容。不要把空输出当成设备没有协议支持的结论。

公开输出前，可遮盖 `PNPDeviceID` 实例路径末段的序列号或蓝牙地址；请保留硬件标识中的 VID、PID、REV、MI、COL、UP、U 等字段，以及同一设备各接口的对应关系。

## 适配中可能遇到的问题

- **型号、固件与模式差异**：同品牌、相似名称、相同 VID，甚至同型号不同固件，也可能使用不同协议。2.4G、蓝牙、USB 模式要分别验证；替换或高回报率接收器也可能改变接口。
- **驱动占用**：厂商驱动或网页驱动可能占用通信接口。请记录它运行、退出时的结果；网页驱动需关闭相关标签页。未测试请注明，不要把猜测写成结论。
- **休眠与在线判断**：外设休眠、关机后接收器仍可能存在；Windows 也可能保留配对节点或历史读数。请记录休眠前、唤醒刷新后及关机后的表现。
- **电量与充电语义**：部分设备只上报分档电量，不提供充电状态，或者充电时暂时返回旧值。请对照官方驱动记录插拔充电线后的变化，区分真实的 0%、未知电量和停止更新。
- **共享接收器与多设备**：同一接收器可能连接多个外设。必须确认电量属于哪个设备，不能仅凭接收器存在或同名条目认定型号与电量归属。
- **协议资料与实机验证不足**：本命令提供身份与接口线索，不能直接给出完整 HID 报告描述符、报告长度、报告 ID 或电量命令。适配仍可能需要分析官方驱动/网页驱动的协议，并请设备持有人验证读数、充电、休眠唤醒和切换模式的表现；提供信息不代表已完成适配。

## English

Use the [English device support form](https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/issues/new?template=device-support.en.yml) for new device requests or identification, battery, charging, and connection problems. Chinese and Russian forms are linked in the language table above; all three use the same fields, required inputs, and collection command.

Include the exact brand/model and variant, an **official driver download or official web driver URL**, the connection mode, receiver model and paired devices, app/Windows/firmware/driver versions, reproduction steps, and the official driver's battery/charging behavior. Mark unknown versions and untested cases explicitly.

Run the PowerShell command above with the device connected and awake. It reads current USB, HID, and common Bluetooth PnP nodes without requiring a known VID/PID or sending device protocol commands. It includes devices with Windows errors, rather than filtering them out. `HardwareID` and `CompatibleID` arrays are expanded to avoid hiding entries behind an ellipsis.

For USB/2.4 GHz devices, compare results with the receiver/cable plugged in and unplugged. Include **all related USB and HID interfaces**, including different `MI_XX` and `COLXX` entries. Turning off the peripheral alone usually leaves its receiver present. Bluetooth pairing nodes can remain after disconnection; also report the connection and battery status shown by Windows.

Paste text output into the form. If the command fails or returns nothing, include the error and the hardware IDs from Device Manager. Serial numbers or Bluetooth addresses in the final instance-path segment may be redacted; preserve VID/PID/REV/MI/COL/UP/U fields and interface relationships.

Compatibility can vary by firmware, receiver, and connection mode. Vendor utilities or web drivers may compete for an interface. Receivers can remain present while peripherals sleep or power off, and some devices report only coarse battery steps or no charging state. Record behavior with the official utility running/exited, during sleep/wake, while connecting/disconnecting a charging cable, and after changing modes; mark untested cases.

These identifiers do **not** include a complete HID report descriptor or battery protocol. Driver analysis and device-owner validation may still be needed before support can be confirmed.

## Русский

Для запроса поддержки нового устройства или сообщения о проблемах с определением, зарядом батареи, зарядкой и подключением используйте [русскую форму](https://github.com/ZGMFX01A/Wireless-Device-Battery-Monitor/issues/new?template=device-support.ru.yml). Китайская и английская формы указаны в таблице языков выше. Во всех трёх формах одинаковые поля, обязательные сведения и команда сбора данных.

Укажите производителя, полную модель и её версию, **официальную ссылку на скачивание драйвера или официальный веб-драйвер**, способ подключения, модель приёмника и сопряжённые устройства, версии приложения/Windows/прошивки/драйвера, шаги воспроизведения и показания заряда/зарядки в официальном драйвере. Неизвестные версии и непроверенные случаи отмечайте явно.

Выполните приведённую выше команду PowerShell при подключённом устройстве, выведенном из спящего режима. Команда читает присутствующие узлы USB, HID и распространённых Bluetooth-устройств; знать VID/PID заранее не нужно. Она не отправляет команды протокола устройства и не исключает устройства с ошибками Windows. Массивы `HardwareID` и `CompatibleID` разворачиваются полностью, без сокращения многоточием.

Для USB / 2,4 ГГц сравните вывод с подключённым приёмником/кабелем и после его отключения. Приложите **все связанные интерфейсы USB и HID**, в том числе разные записи `MI_XX` и `COLXX`. Выключение самого беспроводного устройства обычно не удаляет запись приёмника. Узлы сопряжения Bluetooth могут сохраняться после отключения; дополнительно укажите состояние подключения и заряд, показанные Windows.

Вставьте текст вывода в форму. При ошибке или пустом выводе сообщите об этом, приложите текст ошибки и ИД оборудования из диспетчера устройств. В последнем сегменте пути экземпляра можно скрыть серийный номер или Bluetooth-адрес, сохранив поля VID/PID/REV/MI/COL/UP/U и связь между интерфейсами.

Совместимость зависит от прошивки, приёмника и способа подключения. Утилита производителя или веб-драйвер могут занимать интерфейс связи. Приёмник может оставаться подключённым, когда устройство спит или выключено; некоторые устройства передают заряд крупными шагами либо не сообщают статус зарядки. Опишите поведение при работающей/закрытой официальной утилите, после сна и пробуждения, при подключении/отключении зарядного кабеля и смене способа подключения. Непроверенные случаи отметьте явно.

Эти идентификаторы **не содержат полный дескриптор отчётов HID или протокол чтения заряда**. Для подтверждения поддержки могут потребоваться анализ драйвера и проверка владельцем устройства.
