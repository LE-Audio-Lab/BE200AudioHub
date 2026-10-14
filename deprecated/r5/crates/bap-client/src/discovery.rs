use std::fmt::Display;
use std::time::Duration;

use bumble_adapter::AttachedController;
use bumble_gatt::{CharacteristicProxy, ServiceProxy};
use bumble_profiles::ascs::AudioStreamControlServiceProxy;
use bumble_profiles::cap::CommonAudioServiceProxy;
use bumble_profiles::pacs::PublishedAudioCapabilitiesServiceProxy;
use bumble_transport::{Error, Result as TransportResult};

use crate::ase_read::read_ase_checked;
use crate::snapshot::{
    AseSnapshot, AscsSnapshot, AudioMemberSnapshot, CapSnapshot, ConnectedAudioMember,
    GattServiceRange, PacsSnapshot,
};

pub fn discover_audio_member(
    controller: &mut AttachedController,
    member: &ConnectedAudioMember,
    timeout: Duration,
) -> TransportResult<AudioMemberSnapshot> {
    controller.with_encrypted_gatt(
        member.connection_handle,
        timeout,
        |client, transport| {
            let pacs = PublishedAudioCapabilitiesServiceProxy::discover(client, transport)
                .map_err(|error| profile_error("PACS discovery", error))?
                .ok_or_else(|| Error::Remote("peer does not expose mandatory PACS".into()))?;

            let supported_contexts = pacs
                .read_supported_contexts(client, transport)
                .map_err(|error| profile_error("PACS Supported Audio Contexts read", error))?;
            let available_contexts = pacs
                .read_available_contexts(client, transport)
                .map_err(|error| profile_error("PACS Available Audio Contexts read", error))?;

            let sink_pac_records = match pacs.sink_pac.as_ref() {
                Some(characteristic) => PublishedAudioCapabilitiesServiceProxy::read_pac(
                    characteristic,
                    client,
                    transport,
                )
                .map_err(|error| profile_error("PACS Sink PAC read", error))?,
                None => Vec::new(),
            };
            let source_pac_records = match pacs.source_pac.as_ref() {
                Some(characteristic) => PublishedAudioCapabilitiesServiceProxy::read_pac(
                    characteristic,
                    client,
                    transport,
                )
                .map_err(|error| profile_error("PACS Source PAC read", error))?,
                None => Vec::new(),
            };
            let sink_audio_locations = match pacs.sink_audio_locations.as_ref() {
                Some(characteristic) => Some(
                    PublishedAudioCapabilitiesServiceProxy::read_audio_locations(
                        characteristic,
                        client,
                        transport,
                    )
                    .map_err(|error| profile_error("PACS Sink Audio Locations read", error))?,
                ),
                None => None,
            };
            let source_audio_locations = match pacs.source_audio_locations.as_ref() {
                Some(characteristic) => Some(
                    PublishedAudioCapabilitiesServiceProxy::read_audio_locations(
                        characteristic,
                        client,
                        transport,
                    )
                    .map_err(|error| profile_error("PACS Source Audio Locations read", error))?,
                ),
                None => None,
            };

            let pacs_snapshot = PacsSnapshot {
                service: service_range(&pacs.service),
                sink_pac_handle: pacs.sink_pac.as_ref().map(|value| value.handle),
                source_pac_handle: pacs.source_pac.as_ref().map(|value| value.handle),
                sink_audio_location_handle: pacs
                    .sink_audio_locations
                    .as_ref()
                    .map(|value| value.handle),
                source_audio_location_handle: pacs
                    .source_audio_locations
                    .as_ref()
                    .map(|value| value.handle),
                available_contexts_handle: pacs.available_audio_contexts.handle,
                supported_contexts_handle: pacs.supported_audio_contexts.handle,
                sink_pac_records,
                source_pac_records,
                sink_audio_locations,
                source_audio_locations,
                available_contexts,
                supported_contexts,
            };

            let ascs = AudioStreamControlServiceProxy::discover(client, transport)
                .map_err(|error| profile_error("ASCS discovery", error))?
                .ok_or_else(|| Error::Remote("peer does not expose mandatory ASCS".into()))?;

            let mut sink_ases = Vec::with_capacity(ascs.sink_ase.len());
            for characteristic in &ascs.sink_ase {
                let status = read_ase_checked(
                    member.connection_handle,
                    characteristic,
                    client,
                    transport,
                )
                .map_err(|error| profile_error("ASCS Sink ASE read", error))?;
                sink_ases.push(ase_snapshot(characteristic, status));
            }

            let mut source_ases = Vec::with_capacity(ascs.source_ase.len());
            for characteristic in &ascs.source_ase {
                let status = read_ase_checked(
                    member.connection_handle,
                    characteristic,
                    client,
                    transport,
                )
                .map_err(|error| profile_error("ASCS Source ASE read", error))?;
                source_ases.push(ase_snapshot(characteristic, status));
            }

            let ascs_snapshot = AscsSnapshot {
                service: service_range(&ascs.service),
                sink_ases,
                source_ases,
                control_point_handle: ascs.ase_control_point.handle,
            };

            let cap = match CommonAudioServiceProxy::discover(client, transport)
                .map_err(|error| profile_error("CAS discovery", error))?
            {
                Some(cap) => {
                    let included_csis = cap
                        .discover_coordinated_set_service(client, transport)
                        .map_err(|error| profile_error("CAS included CSIS discovery", error))?;
                    Some(CapSnapshot {
                        service: service_range(&cap.service),
                        included_csis: included_csis.as_ref().map(service_range),
                    })
                }
                None => None,
            };

            Ok(AudioMemberSnapshot {
                identity_address: member.identity_address.clone(),
                rank: member.rank,
                current_address: member.current_address.clone(),
                current_address_type: member.current_address_type,
                connection_handle: member.connection_handle,
                pacs: pacs_snapshot,
                ascs: ascs_snapshot,
                cap,
            })
        },
    )
}

fn service_range(service: &ServiceProxy) -> GattServiceRange {
    GattServiceRange {
        start_handle: service.handle,
        end_handle: service.end_group_handle,
    }
}

fn ase_snapshot(
    characteristic: &CharacteristicProxy,
    status: bumble_profiles::ascs::AseStatus,
) -> AseSnapshot {
    AseSnapshot {
        declaration_handle: characteristic.declaration_handle,
        value_handle: characteristic.handle,
        end_group_handle: characteristic.end_group_handle,
        properties: characteristic.properties,
        status,
    }
}

fn profile_error(label: &str, error: impl Display) -> Error {
    Error::Remote(format!("{label} failed: {error}"))
}
