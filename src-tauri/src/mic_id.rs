//! ID estável de microfone, direto do WASAPI.
//!
//! ## Por que não basta o `cpal`
//!
//! O `cpal` (a biblioteca de áudio que o resto de `audio.rs` usa) só expõe o
//! **nome amigável** de cada dispositivo — não o ID interno que o Windows
//! mantém pra ele. O nome é ótimo pra mostrar na tela, péssimo pra guardar
//! como preferência: o Windows embute um índice de desambiguação nele
//! (`"Fifine microfone (3- fifine Microphone)"`) que muda sozinho entre
//! sessões, e a pessoa também pode renomear o aparelho nas configurações de
//! som — as duas coisas quebram uma comparação por string, por mais
//! cuidadosa que seja.
//!
//! O ID que o `IMMDevice::GetId()` devolve não sofre nenhuma dessas duas
//! coisas: é o mesmo enquanto o aparelho continuar plugado na mesma porta,
//! não importa como o driver decida nomeá-lo hoje. Ele só muda se o mic for
//! pra outra porta USB — o que é razoável exigir que a pessoa escolha de
//! novo, mesmo cenário de "troquei de aparelho".
//!
//! ## Por que é um módulo à parte
//!
//! Nada disto passa pelo `cpal`: é COM cru contra o WASAPI, o mesmo caminho
//! que o `Device::name()` do `cpal` usa por baixo (dá pra conferir no
//! código-fonte dele — é o mesmo `IPropertyStore` / `DEVPKEY_Device_FriendlyName`
//! / `PROPVARIANT` manual). Ficar num arquivo próprio evita espalhar `unsafe`
//! pelo resto de `audio.rs`, que é onde mora a lógica que só depende do
//! `cpal`.

use anyhow::{Context, Result};
use windows::Win32::Devices::Properties::DEVPKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Variant::VT_LPWSTR;

/// Um microfone do jeito que o Windows o identifica de verdade: um ID que não
/// muda, e o nome amigável do momento (que pode mudar a qualquer hora).
pub struct Endpoint {
    pub id: String,
    pub name: String,
}

/// Garante COM iniciado nesta thread pela duração do guarda.
///
/// `CoInitializeEx` é referência-contada por thread, então convive sem
/// conflito com o `cpal` inicializando COM na mesma thread de áudio — os
/// dois só incrementam o contador da mesma apartment. `RPC_E_CHANGED_MODE`
/// (alguém já inicializou como MTA) não é erro fatal aqui pela mesma razão
/// que não é pro `cpal`: COM cuida da compatibilidade por marshalling.
struct ComGuard(bool);

impl ComGuard {
    fn acquire() -> Self {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(result.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("criar o enumerador de dispositivos de áudio do Windows")
}

/// Lê o nome amigável de um endpoint — mesma dança manual de `PROPVARIANT`
/// que o `cpal` faz em `Device::name()`, porque é a forma que essa versão do
/// `windows-rs` exige pra ler uma string de um `IPropertyStore`.
unsafe fn friendly_name(device: &IMMDevice) -> Result<String> {
    let store = device
        .OpenPropertyStore(STGM_READ)
        .context("abrir o property store do microfone")?;

    let mut valor = store
        .GetValue(&DEVPKEY_Device_FriendlyName as *const _ as *const _)
        .context("ler o nome do microfone")?;

    let variante = &valor.as_raw().Anonymous.Anonymous;
    if variante.vt != VT_LPWSTR.0 {
        anyhow::bail!("property store devolveu um tipo inesperado pro nome do microfone");
    }
    let ptr_utf16 = *(&variante.Anonymous as *const _ as *const *const u16);

    let mut len = 0isize;
    while *ptr_utf16.offset(len) != 0 {
        len += 1;
    }
    let fatia = std::slice::from_raw_parts(ptr_utf16, len as usize);
    let nome = String::from_utf16_lossy(fatia);

    let _ = PropVariantClear(&mut valor);
    Ok(nome)
}

/// Lê o ID estável de um endpoint.
unsafe fn endpoint_id(device: &IMMDevice) -> Result<String> {
    let bruto = device.GetId().context("ler o ID do microfone")?;
    let id = bruto
        .to_string()
        .context("ID do microfone não é UTF-16 válido")?;
    CoTaskMemFree(Some(bruto.as_ptr() as *const _));
    Ok(id)
}

/// Todos os microfones ligados agora, com ID estável e nome amigável.
pub fn list_capture_endpoints() -> Result<Vec<Endpoint>> {
    let _com = ComGuard::acquire();
    unsafe {
        let enumerador = enumerator()?;
        let colecao = enumerador
            .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
            .context("enumerar microfones")?;
        let total = colecao.GetCount().context("contar microfones")?;

        let mut lista = Vec::with_capacity(total as usize);
        for indice in 0..total {
            let device = colecao
                .Item(indice)
                .context("acessar um microfone da lista")?;
            let id = endpoint_id(&device)?;
            let name = friendly_name(&device)?;
            lista.push(Endpoint { id, name });
        }
        Ok(lista)
    }
}

/// O ID do microfone padrão do sistema agora, se houver algum ligado.
pub fn default_capture_id() -> Option<String> {
    let _com = ComGuard::acquire();
    unsafe {
        let enumerador = enumerator().ok()?;
        let device = enumerador.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
        endpoint_id(&device).ok()
    }
}
