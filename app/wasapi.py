"""
Windows audio capture for the FOLO studio, straight on WASAPI through cffi.

- Endpoint capture: the loopback of an output ("what plays on it") or an
  input. Event driven, and every read drains all queued packets, so the
  analysis always runs on the newest audio and never on a backlog. Each
  packet carries its capture time, which gives the real capture latency.
- App capture: process loopback (Windows 10 2004 and later), either only one
  app's audio or everything except one app, whatever output it plays on.
- Audio sessions: the apps that have audio on any output right now, with
  their current peak level.
- MMCSS: the "Pro Audio" scheduling class for the capture thread.
"""

# cffi's C objects have no type information: their fields and functions are
# looked up at run time
# pyright: reportAttributeAccessIssue=false

import threading
import time
import uuid

import cffi
import numpy as np

ffi = cffi.FFI()
ffi.cdef(r"""
typedef long HRESULT;
typedef struct {
    unsigned long  Data1;
    unsigned short Data2;
    unsigned short Data3;
    unsigned char  Data4[8];
} GUID;
""")
# mmreg.h packs the wave formats (WAVEFORMATEX is 18 bytes, EXTENSIBLE 40)
ffi.cdef(
    r"""
typedef struct {
    WORD  wFormatTag;
    WORD  nChannels;
    DWORD nSamplesPerSec;
    DWORD nAvgBytesPerSec;
    WORD  nBlockAlign;
    WORD  wBitsPerSample;
    WORD  cbSize;
} WAVEFORMATEX;
typedef struct {
    WAVEFORMATEX Format;
    WORD  wValidBitsPerSample;
    DWORD dwChannelMask;
    GUID  SubFormat;
} WAVEFORMATEXTENSIBLE;
""",
    packed=True,
)
ffi.cdef(r"""
typedef struct {
    unsigned short vt;
    WORD wReserved1;
    WORD wReserved2;
    WORD wReserved3;
    union {
        wchar_t *pwszVal;
        struct { ULONG cbSize; BYTE *pBlobData; } blob;
        unsigned long long pad[2];
    };
} PROPVARIANT;
typedef struct { GUID fmtid; DWORD pid; } PROPERTYKEY;

typedef struct IUnknown IUnknown;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IUnknown *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IUnknown *This);
    ULONG (__stdcall *Release)(IUnknown *This);
} IUnknownVtbl;
struct IUnknown { const IUnknownVtbl *lpVtbl; };

typedef struct IMMDevice IMMDevice;
typedef struct IMMDeviceCollection IMMDeviceCollection;
typedef struct IMMDeviceEnumerator IMMDeviceEnumerator;
typedef struct IMMNotificationClient IMMNotificationClient;
typedef struct IPropertyStore IPropertyStore;

typedef struct {
    HRESULT (__stdcall *QueryInterface)(IMMDeviceEnumerator *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IMMDeviceEnumerator *This);
    ULONG (__stdcall *Release)(IMMDeviceEnumerator *This);
    HRESULT (__stdcall *EnumAudioEndpoints)(IMMDeviceEnumerator *This, int dataFlow, DWORD stateMask, IMMDeviceCollection **devices);
    HRESULT (__stdcall *GetDefaultAudioEndpoint)(IMMDeviceEnumerator *This, int dataFlow, int role, IMMDevice **endpoint);
    HRESULT (__stdcall *GetDevice)(IMMDeviceEnumerator *This, const wchar_t *id, IMMDevice **device);
    HRESULT (__stdcall *RegisterEndpointNotificationCallback)(IMMDeviceEnumerator *This, IMMNotificationClient *client);
    HRESULT (__stdcall *UnregisterEndpointNotificationCallback)(IMMDeviceEnumerator *This, IMMNotificationClient *client);
} IMMDeviceEnumeratorVtbl;
struct IMMDeviceEnumerator { const IMMDeviceEnumeratorVtbl *lpVtbl; };

/* PROPERTYKEY is passed by value; on x64 that is a pointer to a copy */
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IMMNotificationClient *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IMMNotificationClient *This);
    ULONG (__stdcall *Release)(IMMNotificationClient *This);
    HRESULT (__stdcall *OnDeviceStateChanged)(IMMNotificationClient *This, const wchar_t *id, DWORD state);
    HRESULT (__stdcall *OnDeviceAdded)(IMMNotificationClient *This, const wchar_t *id);
    HRESULT (__stdcall *OnDeviceRemoved)(IMMNotificationClient *This, const wchar_t *id);
    HRESULT (__stdcall *OnDefaultDeviceChanged)(IMMNotificationClient *This, int flow, int role, const wchar_t *id);
    HRESULT (__stdcall *OnPropertyValueChanged)(IMMNotificationClient *This, const wchar_t *id, const void *key);
} IMMNotificationClientVtbl;
struct IMMNotificationClient { const IMMNotificationClientVtbl *lpVtbl; };

typedef struct {
    HRESULT (__stdcall *QueryInterface)(IMMDeviceCollection *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IMMDeviceCollection *This);
    ULONG (__stdcall *Release)(IMMDeviceCollection *This);
    HRESULT (__stdcall *GetCount)(IMMDeviceCollection *This, UINT *count);
    HRESULT (__stdcall *Item)(IMMDeviceCollection *This, UINT index, IMMDevice **device);
} IMMDeviceCollectionVtbl;
struct IMMDeviceCollection { const IMMDeviceCollectionVtbl *lpVtbl; };

typedef struct {
    HRESULT (__stdcall *QueryInterface)(IMMDevice *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IMMDevice *This);
    ULONG (__stdcall *Release)(IMMDevice *This);
    HRESULT (__stdcall *Activate)(IMMDevice *This, const GUID *iid, DWORD clsCtx, PROPVARIANT *params, void **ppv);
    HRESULT (__stdcall *OpenPropertyStore)(IMMDevice *This, DWORD access, IPropertyStore **store);
    HRESULT (__stdcall *GetId)(IMMDevice *This, wchar_t **id);
    HRESULT (__stdcall *GetState)(IMMDevice *This, DWORD *state);
} IMMDeviceVtbl;
struct IMMDevice { const IMMDeviceVtbl *lpVtbl; };

typedef struct {
    HRESULT (__stdcall *QueryInterface)(IPropertyStore *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IPropertyStore *This);
    ULONG (__stdcall *Release)(IPropertyStore *This);
    HRESULT (__stdcall *GetCount)(IPropertyStore *This, DWORD *count);
    HRESULT (__stdcall *GetAt)(IPropertyStore *This, DWORD index, PROPERTYKEY *key);
    HRESULT (__stdcall *GetValue)(IPropertyStore *This, const PROPERTYKEY *key, PROPVARIANT *value);
} IPropertyStoreVtbl;
struct IPropertyStore { const IPropertyStoreVtbl *lpVtbl; };

typedef struct IAudioClient IAudioClient;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioClient *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioClient *This);
    ULONG (__stdcall *Release)(IAudioClient *This);
    HRESULT (__stdcall *Initialize)(IAudioClient *This, int shareMode, DWORD streamFlags, long long bufferDuration, long long periodicity, const WAVEFORMATEX *format, const GUID *sessionGuid);
    HRESULT (__stdcall *GetBufferSize)(IAudioClient *This, UINT32 *frames);
    HRESULT (__stdcall *GetStreamLatency)(IAudioClient *This, long long *latency);
    HRESULT (__stdcall *GetCurrentPadding)(IAudioClient *This, UINT32 *frames);
    HRESULT (__stdcall *IsFormatSupported)(IAudioClient *This, int shareMode, const WAVEFORMATEX *format, WAVEFORMATEX **closest);
    HRESULT (__stdcall *GetMixFormat)(IAudioClient *This, WAVEFORMATEX **format);
    HRESULT (__stdcall *GetDevicePeriod)(IAudioClient *This, long long *defaultPeriod, long long *minimumPeriod);
    HRESULT (__stdcall *Start)(IAudioClient *This);
    HRESULT (__stdcall *Stop)(IAudioClient *This);
    HRESULT (__stdcall *Reset)(IAudioClient *This);
    HRESULT (__stdcall *SetEventHandle)(IAudioClient *This, HANDLE event);
    HRESULT (__stdcall *GetService)(IAudioClient *This, const GUID *riid, void **ppv);
} IAudioClientVtbl;
struct IAudioClient { const IAudioClientVtbl *lpVtbl; };

typedef struct IAudioCaptureClient IAudioCaptureClient;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioCaptureClient *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioCaptureClient *This);
    ULONG (__stdcall *Release)(IAudioCaptureClient *This);
    HRESULT (__stdcall *GetBuffer)(IAudioCaptureClient *This, BYTE **data, UINT32 *frames, DWORD *flags, UINT64 *devicePosition, UINT64 *qpcPosition);
    HRESULT (__stdcall *ReleaseBuffer)(IAudioCaptureClient *This, UINT32 frames);
    HRESULT (__stdcall *GetNextPacketSize)(IAudioCaptureClient *This, UINT32 *frames);
} IAudioCaptureClientVtbl;
struct IAudioCaptureClient { const IAudioCaptureClientVtbl *lpVtbl; };

typedef struct IAudioSessionManager2 IAudioSessionManager2;
typedef struct IAudioSessionEnumerator IAudioSessionEnumerator;
typedef struct IAudioSessionControl2 IAudioSessionControl2;
typedef struct IAudioMeterInformation IAudioMeterInformation;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioSessionManager2 *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioSessionManager2 *This);
    ULONG (__stdcall *Release)(IAudioSessionManager2 *This);
    void *GetAudioSessionControl;
    void *GetSimpleAudioVolume;
    HRESULT (__stdcall *GetSessionEnumerator)(IAudioSessionManager2 *This, IAudioSessionEnumerator **sessions);
} IAudioSessionManager2Vtbl;
struct IAudioSessionManager2 { const IAudioSessionManager2Vtbl *lpVtbl; };
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioSessionEnumerator *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioSessionEnumerator *This);
    ULONG (__stdcall *Release)(IAudioSessionEnumerator *This);
    HRESULT (__stdcall *GetCount)(IAudioSessionEnumerator *This, int *count);
    HRESULT (__stdcall *GetSession)(IAudioSessionEnumerator *This, int index, IUnknown **session);
} IAudioSessionEnumeratorVtbl;
struct IAudioSessionEnumerator { const IAudioSessionEnumeratorVtbl *lpVtbl; };
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioSessionControl2 *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioSessionControl2 *This);
    ULONG (__stdcall *Release)(IAudioSessionControl2 *This);
    HRESULT (__stdcall *GetState)(IAudioSessionControl2 *This, int *state);
    void *GetDisplayName;
    void *SetDisplayName;
    void *GetIconPath;
    void *SetIconPath;
    void *GetGroupingParam;
    void *SetGroupingParam;
    void *RegisterAudioSessionNotification;
    void *UnregisterAudioSessionNotification;
    void *GetSessionIdentifier;
    void *GetSessionInstanceIdentifier;
    HRESULT (__stdcall *GetProcessId)(IAudioSessionControl2 *This, DWORD *pid);
    HRESULT (__stdcall *IsSystemSoundsSession)(IAudioSessionControl2 *This);
} IAudioSessionControl2Vtbl;
struct IAudioSessionControl2 { const IAudioSessionControl2Vtbl *lpVtbl; };
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IAudioMeterInformation *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IAudioMeterInformation *This);
    ULONG (__stdcall *Release)(IAudioMeterInformation *This);
    HRESULT (__stdcall *GetPeakValue)(IAudioMeterInformation *This, float *peak);
} IAudioMeterInformationVtbl;
struct IAudioMeterInformation { const IAudioMeterInformationVtbl *lpVtbl; };

typedef struct IActivateAudioInterfaceAsyncOperation IActivateAudioInterfaceAsyncOperation;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IActivateAudioInterfaceAsyncOperation *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IActivateAudioInterfaceAsyncOperation *This);
    ULONG (__stdcall *Release)(IActivateAudioInterfaceAsyncOperation *This);
    HRESULT (__stdcall *GetActivateResult)(IActivateAudioInterfaceAsyncOperation *This, HRESULT *result, IUnknown **activated);
} IActivateAudioInterfaceAsyncOperationVtbl;
struct IActivateAudioInterfaceAsyncOperation { const IActivateAudioInterfaceAsyncOperationVtbl *lpVtbl; };

typedef struct IActivateAudioInterfaceCompletionHandler IActivateAudioInterfaceCompletionHandler;
typedef struct {
    HRESULT (__stdcall *QueryInterface)(IActivateAudioInterfaceCompletionHandler *This, const GUID *riid, void **ppv);
    ULONG (__stdcall *AddRef)(IActivateAudioInterfaceCompletionHandler *This);
    ULONG (__stdcall *Release)(IActivateAudioInterfaceCompletionHandler *This);
    HRESULT (__stdcall *ActivateCompleted)(IActivateAudioInterfaceCompletionHandler *This, IActivateAudioInterfaceAsyncOperation *operation);
} IActivateAudioInterfaceCompletionHandlerVtbl;
struct IActivateAudioInterfaceCompletionHandler { const IActivateAudioInterfaceCompletionHandlerVtbl *lpVtbl; };

typedef struct {
    int ActivationType;
    DWORD TargetProcessId;
    int ProcessLoopbackMode;
} AUDIOCLIENT_ACTIVATION_PARAMS;

typedef struct {
    DWORD dwSize;
    DWORD cntUsage;
    DWORD th32ProcessID;
    ULONG_PTR th32DefaultHeapID;
    DWORD th32ModuleID;
    DWORD cntThreads;
    DWORD th32ParentProcessID;
    LONG pcPriClassBase;
    DWORD dwFlags;
    wchar_t szExeFile[260];
} PROCESSENTRY32W;

HRESULT CoInitializeEx(void *reserved, DWORD coInit);
HRESULT CoCreateInstance(const GUID *clsid, IUnknown *outer, DWORD clsCtx, const GUID *iid, void **ppv);
void CoTaskMemFree(void *pv);
HRESULT PropVariantClear(PROPVARIANT *pv);

HRESULT ActivateAudioInterfaceAsync(const wchar_t *path, const GUID *riid, PROPVARIANT *params, IActivateAudioInterfaceCompletionHandler *handler, IActivateAudioInterfaceAsyncOperation **operation);

HANDLE CreateEventW(void *attributes, BOOL manualReset, BOOL initialState, const wchar_t *name);
DWORD WaitForSingleObject(HANDLE handle, DWORD milliseconds);
BOOL CloseHandle(HANDLE handle);
BOOL QueryPerformanceCounter(long long *count);
BOOL QueryPerformanceFrequency(long long *frequency);
HANDLE CreateToolhelp32Snapshot(DWORD flags, DWORD pid);
BOOL Process32FirstW(HANDLE snapshot, PROCESSENTRY32W *entry);
BOOL Process32NextW(HANDLE snapshot, PROCESSENTRY32W *entry);
HANDLE OpenProcess(DWORD access, BOOL inherit, DWORD pid);
BOOL QueryFullProcessImageNameW(HANDLE process, DWORD flags, wchar_t *name, DWORD *size);

HANDLE AvSetMmThreadCharacteristicsW(const wchar_t *task, DWORD *taskIndex);
BOOL AvRevertMmThreadCharacteristics(HANDLE handle);

DWORD GetFileVersionInfoSizeW(const wchar_t *path, DWORD *handle);
BOOL GetFileVersionInfoW(const wchar_t *path, DWORD handle, DWORD size, void *data);
BOOL VerQueryValueW(const void *block, const wchar_t *subBlock, void **buffer, UINT *length);
""")

_ole32 = ffi.dlopen('ole32')
_kernel32 = ffi.dlopen('kernel32')
_mmdevapi = ffi.dlopen('mmdevapi')
_avrt = ffi.dlopen('avrt')
_version = ffi.dlopen('version')


def _guid(text):
    ptr = ffi.new('GUID *')
    ffi.memmove(ptr, uuid.UUID(text).bytes_le, 16)
    return ptr


def _guid_bytes(ptr):
    return bytes(ffi.buffer(ptr, 16))


CLSID_MMDeviceEnumerator = _guid('BCDE0395-E52F-467C-8E3D-C4579291692E')
IID_IUnknown = _guid('00000000-0000-0000-C000-000000000046')
IID_IAgileObject = _guid('94EA2B94-E9CC-49E0-C0FF-EE64CA8F5B90')
IID_IMMDeviceEnumerator = _guid('A95664D2-9614-4F35-A746-DE8DB63617E6')
IID_IMMNotificationClient = _guid('7991EEC9-7E89-4D85-8390-6C703CEC60C0')
IID_IAudioClient = _guid('1CB9AD4C-DBFA-4C32-B178-C2F568A703B2')
IID_IAudioCaptureClient = _guid('C8ADBD64-E71E-48A0-A4DE-185C395CD317')
IID_IAudioSessionManager2 = _guid('77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F')
IID_IAudioSessionControl2 = _guid('BFB7FF88-7239-4FC9-8FA2-07C950BE9C6D')
IID_IAudioMeterInformation = _guid('C02216F6-8C67-4B5B-9D00-D008E73E0064')
IID_IActivateAudioInterfaceCompletionHandler = _guid('41D949AB-9862-444A-80F6-C261334DA5EB')
KSDATAFORMAT_SUBTYPE_IEEE_FLOAT = _guid('00000003-0000-0010-8000-00AA00389B71')
PKEY_Device_FriendlyName = ffi.new('PROPERTYKEY *')
PKEY_Device_FriendlyName.fmtid = _guid('A45C254E-DF1C-4EFD-8020-67D146A850E0')[0]
PKEY_Device_FriendlyName.pid = 14

CLSCTX_ALL = 23
E_RENDER, E_CAPTURE = 0, 1
DEVICE_STATE_ACTIVE = 1
VT_LPWSTR, VT_BLOB = 31, 65
AUDCLNT_SHAREMODE_SHARED = 0
AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000
AUDCLNT_STREAMFLAGS_EVENTCALLBACK = 0x00040000
AUDCLNT_STREAMFLAGS_NOPERSIST = 0x00080000
AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY = 0x08000000
AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM = 0x80000000
AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY = 0x1
AUDCLNT_BUFFERFLAGS_SILENT = 0x2
AUDCLNT_S_BUFFER_EMPTY = 0x08890001
AUDCLNT_E_DEVICE_INVALIDATED = 0x88890004
AUDCLNT_ACTIVATION_TYPE_PROCESS_LOOPBACK = 1
PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE = 0
PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE = 1
VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK = 'VAD\\Process_Loopback'
WAVE_FORMAT_IEEE_FLOAT = 3
WAVE_FORMAT_EXTENSIBLE = 0xFFFE
WAIT_OBJECT_0 = 0
BUFFER_DURATION = 1000000  # 100 ms in 100 ns units: room for hiccups, read as soon as it lands
S_OK, E_NOINTERFACE = 0, 0x80004002

_ERRORS = {
    0x80004001: 'not implemented',
    0x80004002: 'interface not supported',
    0x80070005: 'access denied',
    0x80070057: 'invalid argument',
    0x80070490: 'not found',
    0x88890001: 'not initialized',
    0x88890004: 'the device was unplugged or disabled',
    0x88890008: 'format not supported',
    0x8889000A: 'the device is in use',
    0x88890021: 'stream flags not supported',
}


class WasapiError(RuntimeError):
    def __init__(self, what, hr):
        self.hr = hr & 0xFFFFFFFF
        detail = _ERRORS.get(self.hr)
        RuntimeError.__init__(
            self, '{} failed (0x{:08X}{})'.format(what, self.hr, ', ' + detail if detail else '')
        )

    @property
    def device_lost(self):
        return self.hr == AUDCLNT_E_DEVICE_INVALIDATED


def _check(hr, what):
    if hr < 0:
        raise WasapiError(what, hr)
    return hr


def _release(ptr):
    if ptr != ffi.NULL:
        ptr.lpVtbl.Release(ptr)


def init_com():
    """COM, multithreaded, for the calling thread (fine to call again)."""
    _ole32.CoInitializeEx(ffi.NULL, 0)


_qpc_frequency = ffi.new('long long *')
_kernel32.QueryPerformanceFrequency(_qpc_frequency)
_qpc_now_ptr = ffi.new('long long *')


def qpc_now():
    """The performance counter in 100 ns units, the clock of packet times."""
    _kernel32.QueryPerformanceCounter(_qpc_now_ptr)
    return _qpc_now_ptr[0] * 10000000 // _qpc_frequency[0]


def boost_thread():
    """
    Puts the calling thread in MMCSS's "Pro Audio" class, so a busy machine
    doesn't make it late for its packets. Returns a handle for unboost().
    """
    index = ffi.new('DWORD *')
    handle = _avrt.AvSetMmThreadCharacteristicsW('Pro Audio', index)
    return handle if handle != ffi.NULL else None


def unboost(handle):
    if handle is not None:
        _avrt.AvRevertMmThreadCharacteristics(handle)


# ---------------------------------------------------------------- endpoints


class _Enumerator:
    def __enter__(self):
        init_com()
        pp = ffi.new('IMMDeviceEnumerator **')
        _check(
            _ole32.CoCreateInstance(
                CLSID_MMDeviceEnumerator,
                ffi.NULL,
                CLSCTX_ALL,
                IID_IMMDeviceEnumerator,
                ffi.cast('void **', pp),
            ),
            'Creating the device enumerator',
        )
        self.ptr = pp[0]
        return self

    def __exit__(self, *exc):
        _release(self.ptr)

    def device(self, device_id):
        pp = ffi.new('IMMDevice **')
        _check(self.ptr.lpVtbl.GetDevice(self.ptr, device_id, pp), 'Finding the device')
        return pp[0]

    def default(self, flow):
        pp = ffi.new('IMMDevice **')
        hr = self.ptr.lpVtbl.GetDefaultAudioEndpoint(self.ptr, flow, 0, pp)
        return pp[0] if hr >= 0 else None

    def devices(self, flow):
        pp = ffi.new('IMMDeviceCollection **')
        _check(
            self.ptr.lpVtbl.EnumAudioEndpoints(self.ptr, flow, DEVICE_STATE_ACTIVE, pp),
            'Listing devices',
        )
        collection = pp[0]
        try:
            count = ffi.new('UINT *')
            _check(collection.lpVtbl.GetCount(collection, count), 'Counting devices')
            for i in range(count[0]):
                device = ffi.new('IMMDevice **')
                if collection.lpVtbl.Item(collection, i, device) >= 0:
                    yield device[0]
        finally:
            _release(collection)


def _device_id(device):
    pp = ffi.new('wchar_t **')
    _check(device.lpVtbl.GetId(device, pp), 'Reading the device id')
    try:
        return ffi.string(pp[0])
    finally:
        _ole32.CoTaskMemFree(pp[0])


def _device_name(device):
    store = ffi.new('IPropertyStore **')
    if device.lpVtbl.OpenPropertyStore(device, 0, store) < 0:
        return ''
    value = ffi.new('PROPVARIANT *')
    try:
        if (
            store[0].lpVtbl.GetValue(store[0], PKEY_Device_FriendlyName, value) >= 0
            and value.vt == VT_LPWSTR
        ):
            return ffi.string(value.pwszVal)
        return ''
    finally:
        _ole32.PropVariantClear(value)
        _release(store[0])


def endpoints():
    """
    Active outputs and inputs: [{'id', 'name', 'output': bool}], plus the
    id of the default output (or None).
    """
    found = []
    default_id = None
    with _Enumerator() as enum:
        default = enum.default(E_RENDER)
        if default is not None:
            default_id = _device_id(default)
            _release(default)
        for flow in (E_RENDER, E_CAPTURE):
            for device in enum.devices(flow):
                try:
                    found.append(
                        {
                            'id': _device_id(device),
                            'name': _device_name(device),
                            'output': flow == E_RENDER,
                        }
                    )
                finally:
                    _release(device)
    return found, default_id


def default_output_id():
    """The id of the default output, or None (takes about 2 ms)."""
    with _Enumerator() as enum:
        device = enum.default(E_RENDER)
        if device is None:
            return None
        try:
            return _device_id(device)
        finally:
            _release(device)


class _DeviceWatcher:
    """
    Windows calls this back (IMMNotificationClient) when the default output
    changes or a device is added, removed, enabled or disabled; each of
    those bumps `serial`. Lives as long as the process.
    """

    def __init__(self):
        self.serial = 0
        self.vtbl = ffi.new('IMMNotificationClientVtbl *')
        self.obj = ffi.new('IMMNotificationClient *')
        known = {_guid_bytes(IID_IUnknown), _guid_bytes(IID_IMMNotificationClient)}

        def query_interface(this, riid, ppv):
            if _guid_bytes(riid) in known:
                ppv[0] = this
                return S_OK
            ppv[0] = ffi.NULL
            return E_NOINTERFACE - 2**32

        def bump(*args):
            self.serial += 1
            return S_OK

        def default_changed(this, flow, role, device_id):
            if flow == E_RENDER and role == 0:  # the console role: what "default" means
                self.serial += 1
            return S_OK

        vt = self.vtbl
        self._callbacks = [
            ffi.callback(ffi.typeof(vt.QueryInterface), query_interface),
            ffi.callback(ffi.typeof(vt.AddRef), lambda this: 1),
            ffi.callback(ffi.typeof(vt.Release), lambda this: 1),
            ffi.callback(ffi.typeof(vt.OnDeviceStateChanged), bump),
            ffi.callback(ffi.typeof(vt.OnDeviceAdded), bump),
            ffi.callback(ffi.typeof(vt.OnDeviceRemoved), bump),
            ffi.callback(ffi.typeof(vt.OnDefaultDeviceChanged), default_changed),
            ffi.callback(ffi.typeof(vt.OnPropertyValueChanged), lambda this, device_id, key: S_OK),
        ]
        (
            vt.QueryInterface,
            vt.AddRef,
            vt.Release,
            vt.OnDeviceStateChanged,
            vt.OnDeviceAdded,
            vt.OnDeviceRemoved,
            vt.OnDefaultDeviceChanged,
            vt.OnPropertyValueChanged,
        ) = self._callbacks
        self.obj.lpVtbl = vt
        self.enum = _Enumerator().__enter__()
        _check(
            self.enum.ptr.lpVtbl.RegisterEndpointNotificationCallback(self.enum.ptr, self.obj),
            'Watching for device changes',
        )


_watcher = None
_watcher_lock = threading.Lock()


def device_changes():
    """
    A counter that goes up whenever the default output changes or a device
    is added, removed, enabled or disabled. None if Windows won't notify;
    callers then poll default_output_id() instead.
    """
    global _watcher
    with _watcher_lock:
        if _watcher is None:
            try:
                _watcher = _DeviceWatcher()
            except Exception:  # pylint: disable=broad-except
                _watcher = False
    return _watcher.serial if _watcher else None


# ---------------------------------------------------------------- processes


def processes():
    """{pid: (exe file name, parent pid)} of all running processes."""
    snapshot = _kernel32.CreateToolhelp32Snapshot(2, 0)  # TH32CS_SNAPPROCESS
    if snapshot == ffi.cast('HANDLE', -1):
        return {}
    found = {}
    try:
        entry = ffi.new('PROCESSENTRY32W *')
        entry.dwSize = ffi.sizeof('PROCESSENTRY32W')
        ok = _kernel32.Process32FirstW(snapshot, entry)
        while ok:
            found[entry.th32ProcessID] = (ffi.string(entry.szExeFile), entry.th32ParentProcessID)
            ok = _kernel32.Process32NextW(snapshot, entry)
    finally:
        _kernel32.CloseHandle(snapshot)
    return found


def app_root(exe, procs=None):
    """
    The pid of the top process of a running app (by exe file name, any
    case): one whose parent isn't the same app, so capturing its process tree
    includes every helper process the app plays sound from. None if the app
    isn't running.
    """
    procs = procs if procs is not None else processes()
    wanted = exe.lower()
    pids = sorted(pid for pid, (name, _) in procs.items() if name.lower() == wanted)
    for pid in pids:
        parent = procs.get(procs[pid][1])
        if parent is None or parent[0].lower() != wanted:
            return pid
    return pids[0] if pids else None


_app_names = {}


def known_app_name(exe):
    """The friendly name of an app seen before, else its exe name without .exe."""
    key = exe.lower()
    return _app_names.get(key) or (exe[:-4] if key.endswith('.exe') else exe)


def app_name(pid, exe):
    """An app's friendly name from its file description ("Spotify"), else its exe name."""
    key = exe.lower()
    if key in _app_names:
        return _app_names[key]
    name = exe[:-4] if exe.lower().endswith('.exe') else exe
    process = _kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if process != ffi.NULL:
        try:
            path = ffi.new('wchar_t[1024]')
            size = ffi.new('DWORD *', 1024)
            if _kernel32.QueryFullProcessImageNameW(process, 0, path, size):
                description = _file_description(ffi.string(path))
                if description:
                    name = description
        finally:
            _kernel32.CloseHandle(process)
    _app_names[key] = name
    return name


def _file_description(path):
    handle = ffi.new('DWORD *')
    size = _version.GetFileVersionInfoSizeW(path, handle)
    if not size:
        return ''
    data = ffi.new('char[]', size)
    if not _version.GetFileVersionInfoW(path, 0, size, data):
        return ''
    value = ffi.new('void **')
    length = ffi.new('UINT *')
    languages = ['040904b0', '040904e4', '000004b0']
    if (
        _version.VerQueryValueW(data, '\\VarFileInfo\\Translation', value, length)
        and length[0] >= 4
    ):
        words = ffi.cast('unsigned short *', value[0])
        languages.insert(0, f'{words[0]:04x}{words[1]:04x}')
    for language in languages:
        if (
            _version.VerQueryValueW(
                data, f'\\StringFileInfo\\{language}\\FileDescription', value, length
            )
            and length[0] > 1
        ):
            return ffi.string(ffi.cast('wchar_t *', value[0])).strip()
    return ''


# ---------------------------------------------------------------- sessions


def sessions():
    """
    Apps with an audio session on any active output:
    {pid: {'active': bool, 'peak': 0..1}}. System sounds are left out.
    """
    found = {}
    with _Enumerator() as enum:
        for device in enum.devices(E_RENDER):
            try:
                _device_sessions(device, found)
            except WasapiError:
                pass
            finally:
                _release(device)
    return found


def _device_sessions(device, found):
    manager = ffi.new('IAudioSessionManager2 **')
    _check(
        device.lpVtbl.Activate(
            device, IID_IAudioSessionManager2, CLSCTX_ALL, ffi.NULL, ffi.cast('void **', manager)
        ),
        'Opening the session manager',
    )
    listing = ffi.new('IAudioSessionEnumerator **')
    try:
        _check(manager[0].lpVtbl.GetSessionEnumerator(manager[0], listing), 'Listing sessions')
        count = ffi.new('int *')
        _check(listing[0].lpVtbl.GetCount(listing[0], count), 'Counting sessions')
        for i in range(count[0]):
            session = ffi.new('IUnknown **')
            if listing[0].lpVtbl.GetSession(listing[0], i, session) < 0:
                continue
            control = ffi.new('IAudioSessionControl2 **')
            meter = ffi.new('IAudioMeterInformation **')
            try:
                if (
                    session[0].lpVtbl.QueryInterface(
                        session[0], IID_IAudioSessionControl2, ffi.cast('void **', control)
                    )
                    < 0
                ):
                    continue
                if control[0].lpVtbl.IsSystemSoundsSession(control[0]) == S_OK:
                    continue
                pid = ffi.new('DWORD *')
                state = ffi.new('int *')
                control[0].lpVtbl.GetProcessId(control[0], pid)
                control[0].lpVtbl.GetState(control[0], state)
                if not pid[0] or state[0] == 2:  # no single process, or expired
                    continue
                peak = ffi.new('float *')
                if (
                    session[0].lpVtbl.QueryInterface(
                        session[0], IID_IAudioMeterInformation, ffi.cast('void **', meter)
                    )
                    >= 0
                ):
                    meter[0].lpVtbl.GetPeakValue(meter[0], peak)
                entry = found.setdefault(pid[0], {'active': False, 'peak': 0.0})
                entry['active'] = entry['active'] or state[0] == 1
                entry['peak'] = max(entry['peak'], float(peak[0]))
            finally:
                if meter[0] != ffi.NULL:
                    _release(meter[0])
                if control[0] != ffi.NULL:
                    _release(control[0])
                _release(session[0])
    finally:
        if listing[0] != ffi.NULL:
            _release(listing[0])
        _release(manager[0])


# ---------------------------------------------------------------- capture


class CaptureStream:
    """
    A running WASAPI capture stream. wait() until audio may be there, then
    read() everything that has arrived.
    """

    def __init__(self, client, channels, rate, event, name):
        self.client = client
        self.channels = channels
        self.rate = rate
        self.event = event
        self.name = name
        period_default = ffi.new('long long *')
        period_min = ffi.new('long long *')
        if (
            client.lpVtbl.GetDevicePeriod(client, period_default, period_min) >= 0
            and period_default[0] > 0
        ):
            self.period = period_default[0] / 1e7
        else:
            self.period = 0.01
        capture = ffi.new('IAudioCaptureClient **')
        _check(
            client.lpVtbl.GetService(client, IID_IAudioCaptureClient, ffi.cast('void **', capture)),
            'Opening the capture client',
        )
        self.capture = capture[0]
        self._data = ffi.new('BYTE **')
        self._frames = ffi.new('UINT32 *')
        self._flags = ffi.new('DWORD *')
        self._qpc = ffi.new('UINT64 *')
        _check(client.lpVtbl.Start(client), 'Starting the stream')

    @property
    def event_driven(self):
        return self.event is not None

    def wait(self, timeout):
        """Blocks until WASAPI signals a packet, or `timeout` seconds."""
        if self.event is None:
            time.sleep(min(timeout, self.period * 0.5))
            return
        _kernel32.WaitForSingleObject(self.event, max(1, int(timeout * 1000)))

    def read(self):
        """
        Every packet waiting, oldest first: [(frames x channels float32,
        capture time of the first frame in 100 ns units, flags)].
        """
        packets = []
        capture = self.capture
        while True:
            # GetBuffer says when the queue is empty, so no packet size calls are needed
            hr = capture.lpVtbl.GetBuffer(
                capture, self._data, self._frames, self._flags, ffi.NULL, self._qpc
            )
            if hr == AUDCLNT_S_BUFFER_EMPTY:
                return packets
            _check(hr, 'Reading audio')
            frames, flags = self._frames[0], self._flags[0]
            if flags & AUDCLNT_BUFFERFLAGS_SILENT or self._data[0] == ffi.NULL:
                block = np.zeros((frames, self.channels), dtype=np.float32)
            else:
                block = (
                    np.frombuffer(
                        ffi.buffer(self._data[0], frames * 4 * self.channels), dtype=np.float32
                    )
                    .reshape(frames, self.channels)
                    .copy()
                )
            capture.lpVtbl.ReleaseBuffer(capture, frames)
            packets.append((block, self._qpc[0], flags))

    def close(self):
        if self.client is None:
            return
        self.client.lpVtbl.Stop(self.client)
        _release(self.capture)
        _release(self.client)
        if self.event is not None:
            _kernel32.CloseHandle(self.event)
        self.client = None


def _float_format(channels, rate, channel_mask=0):
    fmt = ffi.new('WAVEFORMATEXTENSIBLE *')
    fmt.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE
    fmt.Format.nChannels = channels
    fmt.Format.nSamplesPerSec = rate
    fmt.Format.nAvgBytesPerSec = rate * channels * 4
    fmt.Format.nBlockAlign = channels * 4
    fmt.Format.wBitsPerSample = 32
    fmt.Format.cbSize = 22
    fmt.wValidBitsPerSample = 32
    fmt.dwChannelMask = channel_mask
    fmt.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT[0]
    return fmt


def _start(client, stream_flags, fmt, channels, rate, name, use_event=True):
    """Initializes an IAudioClient (consumed) and returns the CaptureStream."""
    if use_event:
        stream_flags |= AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    try:
        _check(
            client.lpVtbl.Initialize(
                client,
                AUDCLNT_SHAREMODE_SHARED,
                stream_flags,
                BUFFER_DURATION,
                0,
                ffi.cast('WAVEFORMATEX *', fmt),
                ffi.NULL,
            ),
            'Opening the stream',
        )
        event = None
        if use_event:
            event = _kernel32.CreateEventW(ffi.NULL, False, False, ffi.NULL)
            _check(client.lpVtbl.SetEventHandle(client, event), 'Setting the stream event')
        return CaptureStream(client, channels, rate, event, name)
    except Exception:
        _release(client)
        raise


def open_endpoint(device_id, loopback, rate=48000):
    """
    Captures an output (loopback=True: what plays on it) or an input, as
    float32 at `rate`, with the device's channels.
    """
    for use_event in (True, False):
        with _Enumerator() as enum:
            device = enum.device(device_id)
            try:
                name = _device_name(device)
                pp = ffi.new('IAudioClient **')
                _check(
                    device.lpVtbl.Activate(
                        device, IID_IAudioClient, CLSCTX_ALL, ffi.NULL, ffi.cast('void **', pp)
                    ),
                    'Opening the device',
                )
            finally:
                _release(device)
        client = pp[0]
        mix = ffi.new('WAVEFORMATEX **')
        _check(client.lpVtbl.GetMixFormat(client, mix), 'Reading the device format')
        channels = mix[0].nChannels
        mask = 0
        if mix[0].wFormatTag == WAVE_FORMAT_EXTENSIBLE:
            mask = ffi.cast('WAVEFORMATEXTENSIBLE *', mix[0]).dwChannelMask
        _ole32.CoTaskMemFree(mix[0])
        flags = (
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
            | AUDCLNT_STREAMFLAGS_NOPERSIST
            | (AUDCLNT_STREAMFLAGS_LOOPBACK if loopback else 0)
        )
        try:
            return _start(
                client, flags, _float_format(channels, rate, mask), channels, rate, name, use_event
            )
        except WasapiError as exc:
            # Older systems refuse event driven loopback: poll instead
            if not use_event or exc.device_lost:
                raise
    raise RuntimeError('unreachable')


class _ActivationHandler:
    """
    The COM object ActivateAudioInterfaceAsync calls back when the process
    loopback client is ready. Agile, as the API requires.
    """

    def __init__(self):
        self.done = threading.Event()
        self.vtbl = ffi.new('IActivateAudioInterfaceCompletionHandlerVtbl *')
        self.obj = ffi.new('IActivateAudioInterfaceCompletionHandler *')
        self._callbacks = [
            ffi.callback(ffi.typeof(self.vtbl.QueryInterface), self._query_interface),
            ffi.callback(ffi.typeof(self.vtbl.AddRef), lambda this: 1),
            ffi.callback(ffi.typeof(self.vtbl.Release), lambda this: 1),
            ffi.callback(ffi.typeof(self.vtbl.ActivateCompleted), self._completed),
        ]
        (
            self.vtbl.QueryInterface,
            self.vtbl.AddRef,
            self.vtbl.Release,
            self.vtbl.ActivateCompleted,
        ) = self._callbacks
        self.obj.lpVtbl = self.vtbl
        self._known = {
            _guid_bytes(g)
            for g in (IID_IUnknown, IID_IAgileObject, IID_IActivateAudioInterfaceCompletionHandler)
        }

    def _query_interface(self, this, riid, ppv):
        if _guid_bytes(riid) in self._known:
            ppv[0] = this
            return S_OK
        ppv[0] = ffi.NULL
        return E_NOINTERFACE - 2**32

    def _completed(self, this, operation):
        self.done.set()
        return S_OK


def open_app(pid, include=True, rate=48000, channels=2):
    """
    Captures one app's audio and its helper processes (include=True), or
    everything except that app (include=False), whatever output it plays
    on. Needs Windows 10 2004 or later.
    """
    init_com()
    params = ffi.new('AUDIOCLIENT_ACTIVATION_PARAMS *')
    params.ActivationType = AUDCLNT_ACTIVATION_TYPE_PROCESS_LOOPBACK
    params.TargetProcessId = pid
    params.ProcessLoopbackMode = (
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
        if include
        else PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
    )
    variant = ffi.new('PROPVARIANT *')
    variant.vt = VT_BLOB
    variant.blob.cbSize = ffi.sizeof('AUDIOCLIENT_ACTIVATION_PARAMS')
    variant.blob.pBlobData = ffi.cast('BYTE *', params)
    handler = _ActivationHandler()
    operation = ffi.new('IActivateAudioInterfaceAsyncOperation **')
    _check(
        _mmdevapi.ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, IID_IAudioClient, variant, handler.obj, operation
        ),
        'Starting app capture',
    )
    try:
        if not handler.done.wait(5):
            raise RuntimeError('App capture did not start')
        result = ffi.new('HRESULT *')
        activated = ffi.new('IUnknown **')
        _check(
            operation[0].lpVtbl.GetActivateResult(operation[0], result, activated),
            'Starting app capture',
        )
        _check(result[0], 'Starting app capture')
    finally:
        _release(operation[0])
    pp = ffi.new('IAudioClient **')
    try:
        _check(
            activated[0].lpVtbl.QueryInterface(
                activated[0], IID_IAudioClient, ffi.cast('void **', pp)
            ),
            'Opening app capture',
        )
    finally:
        _release(activated[0])
    fmt = ffi.new('WAVEFORMATEX *')
    fmt.wFormatTag = WAVE_FORMAT_IEEE_FLOAT
    fmt.nChannels = channels
    fmt.nSamplesPerSec = rate
    fmt.nAvgBytesPerSec = rate * channels * 4
    fmt.nBlockAlign = channels * 4
    fmt.wBitsPerSample = 32
    fmt.cbSize = 0
    # Process loopback takes only these flags (as in Microsoft's sample)
    stream = _start(
        pp[0],
        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        fmt,
        channels,
        rate,
        '',
    )
    stream._handler = handler  # the callbacks must outlive the activation
    return stream
