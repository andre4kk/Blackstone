// Generato da tools/protocolgen.py dagli schemi Mojang (protocollo 2225). NON MODIFICARE A MANO: rigenerare.
// Contiene solo la codifica dei pacchetti server -> client. Campi `Vec<u8>` = dati già codificati (NBT, oneOf).
#![allow(clippy::all, dead_code)]
use crate::item::ItemStack;
use crate::wire::{Encode, ServerPacket, Writer};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Encode for Vec3 {
    fn encode(&self, w: &mut Writer) {
        w.f32(self.x);
        w.f32(self.y);
        w.f32(self.z);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Encode for Vec2 {
    fn encode(&self, w: &mut Writer) {
        w.f32(self.x);
        w.f32(self.y);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpawnSettings {
    pub spawn_biome_type: i16,
    pub user_defined_biome_name: String,
    pub dimension: i32,
}

impl Encode for SpawnSettings {
    fn encode(&self, w: &mut Writer) {
        w.i16(self.spawn_biome_type);
        w.string(&self.user_defined_biome_name);
        w.var_i32(self.dimension);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Encode for BlockPos {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.x);
        w.var_i32(self.y);
        w.var_i32(self.z);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameRule {
    pub rule_name: String,
    pub rule_can_be_modified: bool,
    pub rule_value: Vec<u8>,
}

impl Encode for GameRule {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.rule_name);
        w.bool(self.rule_can_be_modified);
        w.raw(&self.rule_value);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameRulesChangedPacketData {
    pub rules_list: Vec<GameRule>,
}

impl Encode for GameRulesChangedPacketData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.rules_list.len() as u32);
        for v in &self.rules_list {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExperimentToggle {
    pub name: String,
    pub enabled: bool,
}

impl Encode for ExperimentToggle {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.name);
        w.bool(self.enabled);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Experiments {
    pub toggles: Vec<ExperimentToggle>,
    pub experiments_ever_toggled: bool,
}

impl Encode for Experiments {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.toggles.len() as u32);
        for v in &self.toggles {
            v.encode(w);
        }
        w.bool(self.experiments_ever_toggled);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EduSharedUriResource {
    pub button_name: String,
    pub link_uri: String,
}

impl Encode for EduSharedUriResource {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.button_name);
        w.string(&self.link_uri);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelSettings {
    pub seed: u64,
    pub spawn_settings: SpawnSettings,
    pub generator_type: i32,
    pub game_type: i32,
    pub is_hardcore: bool,
    pub game_difficulty: i32,
    pub default_spawn_block_position: BlockPos,
    pub achievements_disabled: bool,
    pub editor_world_type: i32,
    pub is_created_in_editor: bool,
    pub is_exported_from_editor: bool,
    pub editor_level_migration_version: i8,
    pub day_cycle_stop_time: i32,
    pub education_edition_offer: u32,
    pub education_features_enabled: bool,
    pub education_product_id: String,
    pub rain_level: f32,
    pub lightning_level: f32,
    pub has_confirmed_platform_locked_content: bool,
    pub multiplayer_game_intent: bool,
    pub lan_broadcast_intent: bool,
    pub xbox_live_broadcast_setting: i32,
    pub platform_broadcast_setting: i32,
    pub commands_enabled: bool,
    pub texture_packs_required: bool,
    pub rule_data: GameRulesChangedPacketData,
    pub experiments: Experiments,
    pub has_bonus_chest_enabled: bool,
    pub start_with_map_enabled: bool,
    pub player_permissions: i8,
    pub server_chunk_tick_range: i32,
    pub has_locked_behavior_pack: bool,
    pub has_locked_resource_pack: bool,
    pub is_from_locked_template: bool,
    pub use_msa_gamertags_only: bool,
    pub is_from_world_template: bool,
    pub is_world_template_option_locked: bool,
    pub only_spawn_v1_villagers: bool,
    pub persona_disabled: bool,
    pub custom_skins_disabled: bool,
    pub emote_chat_muted: bool,
    pub base_game_version: String,
    pub limited_world_width: i32,
    pub limited_world_depth: i32,
    pub nether_type: bool,
    pub edu_shared_uri_resource: EduSharedUriResource,
    pub override_force_experimental_gameplay: Option<bool>,
    pub chat_restriction_level: u8,
    pub disable_player_interactions: bool,
    pub server_editor_connection_policy: i32,
    pub allow_anonymous_block_drops_in_editor_worlds: bool,
}

impl Encode for LevelSettings {
    fn encode(&self, w: &mut Writer) {
        w.u64(self.seed);
        self.spawn_settings.encode(w);
        w.var_i32(self.generator_type);
        w.var_i32(self.game_type);
        w.bool(self.is_hardcore);
        w.var_i32(self.game_difficulty);
        self.default_spawn_block_position.encode(w);
        w.bool(self.achievements_disabled);
        w.var_i32(self.editor_world_type);
        w.bool(self.is_created_in_editor);
        w.bool(self.is_exported_from_editor);
        w.i8(self.editor_level_migration_version);
        w.var_i32(self.day_cycle_stop_time);
        w.var_u32(self.education_edition_offer);
        w.bool(self.education_features_enabled);
        w.string(&self.education_product_id);
        w.f32(self.rain_level);
        w.f32(self.lightning_level);
        w.bool(self.has_confirmed_platform_locked_content);
        w.bool(self.multiplayer_game_intent);
        w.bool(self.lan_broadcast_intent);
        w.var_i32(self.xbox_live_broadcast_setting);
        w.var_i32(self.platform_broadcast_setting);
        w.bool(self.commands_enabled);
        w.bool(self.texture_packs_required);
        self.rule_data.encode(w);
        self.experiments.encode(w);
        w.bool(self.has_bonus_chest_enabled);
        w.bool(self.start_with_map_enabled);
        w.i8(self.player_permissions);
        w.i32(self.server_chunk_tick_range);
        w.bool(self.has_locked_behavior_pack);
        w.bool(self.has_locked_resource_pack);
        w.bool(self.is_from_locked_template);
        w.bool(self.use_msa_gamertags_only);
        w.bool(self.is_from_world_template);
        w.bool(self.is_world_template_option_locked);
        w.bool(self.only_spawn_v1_villagers);
        w.bool(self.persona_disabled);
        w.bool(self.custom_skins_disabled);
        w.bool(self.emote_chat_muted);
        w.string(&self.base_game_version);
        w.i32(self.limited_world_width);
        w.i32(self.limited_world_depth);
        w.bool(self.nether_type);
        self.edu_shared_uri_resource.encode(w);
        match &self.override_force_experimental_gameplay {
            Some(v) => {
                w.u8(1);
                w.bool(*v);
            }
            None => w.u8(0),
        }
        w.u8(self.chat_restriction_level);
        w.bool(self.disable_player_interactions);
        w.var_i32(self.server_editor_connection_policy);
        w.bool(self.allow_anonymous_block_drops_in_editor_worlds);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncedPlayerMovementSettings {
    pub rewind_history_size: i32,
    pub server_authoritative_block_breaking: bool,
}

impl Encode for SyncedPlayerMovementSettings {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.rewind_history_size);
        w.bool(self.server_authoritative_block_breaking);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerBlockProperty {
    pub block_name: String,
    pub block_definition: Vec<u8>,
}

impl Encode for ServerBlockProperty {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.block_name);
        w.raw(&self.block_definition);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MceUUID {
    pub most_significant_bits: u64,
    pub least_significant_bits: u64,
}

impl Encode for MceUUID {
    fn encode(&self, w: &mut Writer) {
        w.u64(self.most_significant_bits);
        w.u64(self.least_significant_bits);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkPermissions {
    pub server_auth_sound_enabled: bool,
}

impl Encode for NetworkPermissions {
    fn encode(&self, w: &mut Writer) {
        w.bool(self.server_auth_sound_enabled);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GatheringsConfig {
    pub experience_id: Option<MceUUID>,
    pub experience_name: Option<String>,
    pub world_id: Option<MceUUID>,
    pub world_name: Option<String>,
    pub creator_id: Option<String>,
    pub target_id: Option<MceUUID>,
    pub scenario_id: Option<String>,
    pub server_id: Option<String>,
}

impl Encode for GatheringsConfig {
    fn encode(&self, w: &mut Writer) {
        match &self.experience_id {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.experience_name {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
        match &self.world_id {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.world_name {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
        match &self.creator_id {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
        match &self.target_id {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.scenario_id {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
        match &self.server_id {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientStoreEntryPointConfig {
    pub store_id: String,
    pub store_name: String,
}

impl Encode for ClientStoreEntryPointConfig {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.store_id);
        w.string(&self.store_name);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresenceConfig {
    pub rich_presence_id: Option<String>,
}

impl Encode for PresenceConfig {
    fn encode(&self, w: &mut Writer) {
        match &self.rich_presence_id {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerConfig {
    pub gathering: Option<GatheringsConfig>,
    pub client_store_entry_point: Option<ClientStoreEntryPointConfig>,
    pub presence: Option<PresenceConfig>,
}

impl Encode for ServerConfig {
    fn encode(&self, w: &mut Writer) {
        match &self.gathering {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.client_store_entry_point {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.presence {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerTelemetryData {
    pub server_id: String,
    pub scenario_id: String,
    pub world_id: String,
    pub owner_id: String,
}

impl Encode for ServerTelemetryData {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.server_id);
        w.string(&self.scenario_id);
        w.string(&self.world_id);
        w.string(&self.owner_id);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartGame {
    pub entity_id: i64,
    pub runtime_id: u64,
    pub game_type: i32,
    pub position: Vec3,
    pub rotation: Vec2,
    pub settings: LevelSettings,
    pub level_id: String,
    pub level_name: String,
    pub template_content_identity: String,
    pub is_trial: bool,
    pub movement_settings: SyncedPlayerMovementSettings,
    pub level_current_time: u64,
    pub enchantment_seed: i32,
    pub block_properties: Vec<ServerBlockProperty>,
    pub multiplayer_correlation_id: String,
    pub enable_item_stack_net_manager: bool,
    pub server_version: String,
    pub player_property_data: Vec<u8>,
    pub server_block_type_registry_checksum: u64,
    pub world_template_id: MceUUID,
    pub server_enabled_client_side_generation: bool,
    pub block_network_ids_are_hashes: bool,
    pub network_permissions: NetworkPermissions,
    pub server_configuration_join_info: Option<ServerConfig>,
    pub server_telemetry_data: ServerTelemetryData,
}

impl Encode for StartGame {
    fn encode(&self, w: &mut Writer) {
        w.var_i64(self.entity_id);
        w.var_u64(self.runtime_id);
        w.var_i32(self.game_type);
        self.position.encode(w);
        self.rotation.encode(w);
        self.settings.encode(w);
        w.string(&self.level_id);
        w.string(&self.level_name);
        w.string(&self.template_content_identity);
        w.bool(self.is_trial);
        self.movement_settings.encode(w);
        w.u64(self.level_current_time);
        w.var_i32(self.enchantment_seed);
        w.var_u32(self.block_properties.len() as u32);
        for v in &self.block_properties {
            v.encode(w);
        }
        w.string(&self.multiplayer_correlation_id);
        w.bool(self.enable_item_stack_net_manager);
        w.string(&self.server_version);
        w.raw(&self.player_property_data);
        w.u64(self.server_block_type_registry_checksum);
        self.world_template_id.encode(w);
        w.bool(self.server_enabled_client_side_generation);
        w.bool(self.block_network_ids_are_hashes);
        self.network_permissions.encode(w);
        match &self.server_configuration_join_info {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        self.server_telemetry_data.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemData {
    pub item_name: String,
    pub item_id: i16,
    pub is_component_based: bool,
    pub item_version: i32,
    pub item_component_data: Vec<u8>,
}

impl Encode for ItemData {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.item_name);
        w.i16(self.item_id);
        w.bool(self.is_component_based);
        w.var_i32(self.item_version);
        w.raw(&self.item_component_data);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemRegistry {
    pub item_data: Vec<ItemData>,
}

impl Encode for ItemRegistry {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.item_data.len() as u32);
        for v in &self.item_data {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl Encode for ChunkPos {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.x);
        w.var_i32(self.z);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelChunk {
    pub chunk_position: ChunkPos,
    pub dimension_id: i32,
    pub sub_chunks_count: u32,
    pub client_request_sub_chunk_limit: Option<i32>,
    pub cache_enabled: bool,
    pub cache_metadata: Vec<u64>,
    pub serialized_chunk_data: Vec<u8>,
    pub is_client_biome_update: bool,
}

impl Encode for LevelChunk {
    fn encode(&self, w: &mut Writer) {
        self.chunk_position.encode(w);
        w.var_i32(self.dimension_id);
        w.var_u32(self.sub_chunks_count);
        match &self.client_request_sub_chunk_limit {
            Some(v) => {
                w.u8(1);
                w.var_i32(*v);
            }
            None => w.u8(0),
        }
        w.bool(self.cache_enabled);
        w.var_u32(self.cache_metadata.len() as u32);
        for v in &self.cache_metadata {
            w.u64(*v);
        }
        w.bytes(&self.serialized_chunk_data);
        w.bool(self.is_client_biome_update);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkRadiusUpdated {
    pub chunk_radius: i32,
}

impl Encode for ChunkRadiusUpdated {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.chunk_radius);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkChunkPublisherUpdate {
    pub new_position_for_view: BlockPos,
    pub new_radius_for_view: u32,
    pub server_built_chunks_list: Vec<ChunkPos>,
}

impl Encode for NetworkChunkPublisherUpdate {
    fn encode(&self, w: &mut Writer) {
        self.new_position_for_view.encode(w);
        w.var_u32(self.new_radius_for_view);
        w.u32(self.server_built_chunks_list.len() as u32);
        for v in &self.server_built_chunks_list {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeTagsData {
    pub tags: Vec<u16>,
}

impl Encode for BiomeTagsData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.tags.len() as u32);
        for v in &self.tags {
            w.u16(*v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeClimateData {
    pub temperature: f32,
    pub downfall: f32,
    pub snow_accumulation_min: f32,
    pub snow_accumulation_max: f32,
}

impl Encode for BiomeClimateData {
    fn encode(&self, w: &mut Writer) {
        w.f32(self.temperature);
        w.f32(self.downfall);
        w.f32(self.snow_accumulation_min);
        w.f32(self.snow_accumulation_max);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeCoordinateData {
    pub min_value_type: i32,
    pub min_value: u16,
    pub max_value_type: i32,
    pub max_value: u16,
    pub grid_offset: u32,
    pub grid_step_size: u32,
    pub distribution: i32,
}

impl Encode for BiomeCoordinateData {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.min_value_type);
        w.u16(self.min_value);
        w.var_i32(self.max_value_type);
        w.u16(self.max_value);
        w.u32(self.grid_offset);
        w.u32(self.grid_step_size);
        w.var_i32(self.distribution);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeScatterParamData {
    pub coordinates: Vec<BiomeCoordinateData>,
    pub eval_order: i32,
    pub chance_percent_type: i32,
    pub chance_percent: u16,
    pub chance_numerator: i32,
    pub chance_denominator: i32,
    pub iterations_type: i32,
    pub iterations: u16,
}

impl Encode for BiomeScatterParamData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.coordinates.len() as u32);
        for v in &self.coordinates {
            v.encode(w);
        }
        w.var_i32(self.eval_order);
        w.var_i32(self.chance_percent_type);
        w.u16(self.chance_percent);
        w.i32(self.chance_numerator);
        w.i32(self.chance_denominator);
        w.var_i32(self.iterations_type);
        w.u16(self.iterations);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeConsolidatedFeatureData {
    pub scatter: BiomeScatterParamData,
    pub feature: u16,
    pub identifier: u16,
    pub pass: u16,
    pub can_use_internal_feature: bool,
}

impl Encode for BiomeConsolidatedFeatureData {
    fn encode(&self, w: &mut Writer) {
        self.scatter.encode(w);
        w.u16(self.feature);
        w.u16(self.identifier);
        w.u16(self.pass);
        w.bool(self.can_use_internal_feature);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeConsolidatedFeaturesData {
    pub features: Vec<BiomeConsolidatedFeatureData>,
}

impl Encode for BiomeConsolidatedFeaturesData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.features.len() as u32);
        for v in &self.features {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeMountainParamsData {
    pub steep_block: u32,
    pub north_slopes: bool,
    pub south_slopes: bool,
    pub west_slopes: bool,
    pub east_slopes: bool,
    pub top_slide_enabled: bool,
}

impl Encode for BiomeMountainParamsData {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.steep_block);
        w.bool(self.north_slopes);
        w.bool(self.south_slopes);
        w.bool(self.west_slopes);
        w.bool(self.east_slopes);
        w.bool(self.top_slide_enabled);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeSurfaceMaterialData {
    pub top_block: u32,
    pub mid_block: u32,
    pub sea_floor_block: u32,
    pub foundation_block: u32,
    pub sea_block: u32,
    pub sea_floor_depth: i32,
}

impl Encode for BiomeSurfaceMaterialData {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.top_block);
        w.u32(self.mid_block);
        w.u32(self.sea_floor_block);
        w.u32(self.foundation_block);
        w.u32(self.sea_block);
        w.i32(self.sea_floor_depth);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeElementData {
    pub noise_freq_scale: f32,
    pub noise_lower_bound: f32,
    pub noise_upper_bound: f32,
    pub height_min_type: i32,
    pub height_min: u16,
    pub height_max_type: i32,
    pub height_max: u16,
    pub adjusted_materials: BiomeSurfaceMaterialData,
}

impl Encode for BiomeElementData {
    fn encode(&self, w: &mut Writer) {
        w.f32(self.noise_freq_scale);
        w.f32(self.noise_lower_bound);
        w.f32(self.noise_upper_bound);
        w.var_i32(self.height_min_type);
        w.u16(self.height_min);
        w.var_i32(self.height_max_type);
        w.u16(self.height_max);
        self.adjusted_materials.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeSurfaceMaterialAdjustmentData {
    pub adjustments: Vec<BiomeElementData>,
}

impl Encode for BiomeSurfaceMaterialAdjustmentData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.adjustments.len() as u32);
        for v in &self.adjustments {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeWeightedData {
    pub biome_identifier: u16,
    pub weight: u32,
}

impl Encode for BiomeWeightedData {
    fn encode(&self, w: &mut Writer) {
        w.u16(self.biome_identifier);
        w.u32(self.weight);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeConditionalTransformationData {
    pub transforms_into: Vec<BiomeWeightedData>,
    pub condition_json: u16,
    pub min_passing_neighbors: u32,
}

impl Encode for BiomeConditionalTransformationData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.transforms_into.len() as u32);
        for v in &self.transforms_into {
            v.encode(w);
        }
        w.u16(self.condition_json);
        w.u32(self.min_passing_neighbors);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeWeightedTemperatureData {
    pub temperature: i32,
    pub weight: u32,
}

impl Encode for BiomeWeightedTemperatureData {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.temperature);
        w.u32(self.weight);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeOverworldGenRulesData {
    pub hills_transformations: Vec<BiomeWeightedData>,
    pub mutate_transformations: Vec<BiomeWeightedData>,
    pub river_transformations: Vec<BiomeWeightedData>,
    pub shore_transformations: Vec<BiomeWeightedData>,
    pub pre_hills_edge: Vec<BiomeConditionalTransformationData>,
    pub post_shore_edge: Vec<BiomeConditionalTransformationData>,
    pub climate: Vec<BiomeWeightedTemperatureData>,
}

impl Encode for BiomeOverworldGenRulesData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.hills_transformations.len() as u32);
        for v in &self.hills_transformations {
            v.encode(w);
        }
        w.var_u32(self.mutate_transformations.len() as u32);
        for v in &self.mutate_transformations {
            v.encode(w);
        }
        w.var_u32(self.river_transformations.len() as u32);
        for v in &self.river_transformations {
            v.encode(w);
        }
        w.var_u32(self.shore_transformations.len() as u32);
        for v in &self.shore_transformations {
            v.encode(w);
        }
        w.var_u32(self.pre_hills_edge.len() as u32);
        for v in &self.pre_hills_edge {
            v.encode(w);
        }
        w.var_u32(self.post_shore_edge.len() as u32);
        for v in &self.post_shore_edge {
            v.encode(w);
        }
        w.var_u32(self.climate.len() as u32);
        for v in &self.climate {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeMultinoiseGenRulesData {
    pub temperature: f32,
    pub humidity: f32,
    pub altitude: f32,
    pub weirdness: f32,
    pub weight: f32,
}

impl Encode for BiomeMultinoiseGenRulesData {
    fn encode(&self, w: &mut Writer) {
        w.f32(self.temperature);
        w.f32(self.humidity);
        w.f32(self.altitude);
        w.f32(self.weirdness);
        w.f32(self.weight);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeLegacyWorldGenRulesData {
    pub legacy_pre_hills_edge: Vec<BiomeConditionalTransformationData>,
}

impl Encode for BiomeLegacyWorldGenRulesData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.legacy_pre_hills_edge.len() as u32);
        for v in &self.legacy_pre_hills_edge {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeReplacementData {
    pub replacement_biome: u16,
    pub dimension: u16,
    pub target_biomes: Vec<u16>,
    pub amount: f32,
    pub noise_frequency_scale: f32,
    pub replacement_index: u32,
}

impl Encode for BiomeReplacementData {
    fn encode(&self, w: &mut Writer) {
        w.u16(self.replacement_biome);
        w.u16(self.dimension);
        w.var_u32(self.target_biomes.len() as u32);
        for v in &self.target_biomes {
            w.u16(*v);
        }
        w.f32(self.amount);
        w.f32(self.noise_frequency_scale);
        w.u32(self.replacement_index);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeReplacementsData {
    pub biome_replacements: Vec<BiomeReplacementData>,
}

impl Encode for BiomeReplacementsData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.biome_replacements.len() as u32);
        for v in &self.biome_replacements {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeMesaSurfaceData {
    pub clay_material: u32,
    pub hard_clay_material: u32,
    pub bryce_pillars: bool,
    pub has_forest: bool,
}

impl Encode for BiomeMesaSurfaceData {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.clay_material);
        w.u32(self.hard_clay_material);
        w.bool(self.bryce_pillars);
        w.bool(self.has_forest);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeCappedSurfaceData {
    pub floor_blocks: Vec<u32>,
    pub ceiling_blocks: Vec<u32>,
    pub sea_block: Option<u32>,
    pub foundation_block: Option<u32>,
    pub beach_block: Option<u32>,
}

impl Encode for BiomeCappedSurfaceData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.floor_blocks.len() as u32);
        for v in &self.floor_blocks {
            w.u32(*v);
        }
        w.var_u32(self.ceiling_blocks.len() as u32);
        for v in &self.ceiling_blocks {
            w.u32(*v);
        }
        match &self.sea_block {
            Some(v) => {
                w.u8(1);
                w.u32(*v);
            }
            None => w.u8(0),
        }
        match &self.foundation_block {
            Some(v) => {
                w.u8(1);
                w.u32(*v);
            }
            None => w.u8(0),
        }
        match &self.beach_block {
            Some(v) => {
                w.u8(1);
                w.u32(*v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloatRange {
    pub min: Option<f32>,
    pub max: Option<f32>,
}

impl Encode for FloatRange {
    fn encode(&self, w: &mut Writer) {
        match &self.min {
            Some(v) => {
                w.u8(1);
                w.f32(*v);
            }
            None => w.u8(0),
        }
        match &self.max {
            Some(v) => {
                w.u8(1);
                w.f32(*v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedNoiseBlockSpecifier {
    pub noise: Option<String>,
    pub threshold: Option<f32>,
    pub range: Option<FloatRange>,
    pub block: Option<u32>,
}

impl Encode for SerializedNoiseBlockSpecifier {
    fn encode(&self, w: &mut Writer) {
        match &self.noise {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
        match &self.threshold {
            Some(v) => {
                w.u8(1);
                w.f32(*v);
            }
            None => w.u8(0),
        }
        match &self.range {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.block {
            Some(v) => {
                w.u8(1);
                w.u32(*v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoiseDescriptor {
    pub name: String,
    pub first_octave: i32,
    pub amplitudes: Vec<f32>,
}

impl Encode for NoiseDescriptor {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.name);
        w.i32(self.first_octave);
        w.var_u32(self.amplitudes.len() as u32);
        for v in &self.amplitudes {
            w.f32(*v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeNoiseGradientSurfaceData {
    pub non_replaceable_blocks: Vec<u32>,
    pub gradient_blocks: Vec<SerializedNoiseBlockSpecifier>,
    pub noise: NoiseDescriptor,
}

impl Encode for BiomeNoiseGradientSurfaceData {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.non_replaceable_blocks.len() as u32);
        for v in &self.non_replaceable_blocks {
            w.u32(*v);
        }
        w.var_u32(self.gradient_blocks.len() as u32);
        for v in &self.gradient_blocks {
            v.encode(w);
        }
        self.noise.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeSurfaceBuilderData {
    pub surface_materials: Option<BiomeSurfaceMaterialData>,
    pub has_default_overworld_surface: bool,
    pub has_swamp_surface: bool,
    pub has_frozen_ocean_surface: bool,
    pub has_the_end_surface: bool,
    pub mesa_surface: Option<BiomeMesaSurfaceData>,
    pub capped_surface: Option<BiomeCappedSurfaceData>,
    pub noise_gradient_surface: Option<BiomeNoiseGradientSurfaceData>,
}

impl Encode for BiomeSurfaceBuilderData {
    fn encode(&self, w: &mut Writer) {
        match &self.surface_materials {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        w.bool(self.has_default_overworld_surface);
        w.bool(self.has_swamp_surface);
        w.bool(self.has_frozen_ocean_surface);
        w.bool(self.has_the_end_surface);
        match &self.mesa_surface {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.capped_surface {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.noise_gradient_surface {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeDefinitionChunkGenData {
    pub climate: Option<BiomeClimateData>,
    pub consolidated_features: Option<BiomeConsolidatedFeaturesData>,
    pub mountain_params: Option<BiomeMountainParamsData>,
    pub surface_material_adjustments: Option<BiomeSurfaceMaterialAdjustmentData>,
    pub overworld_gen_rules: Option<BiomeOverworldGenRulesData>,
    pub multinoise_gen_rules: Option<BiomeMultinoiseGenRulesData>,
    pub legacy_world_gen_rules: Option<BiomeLegacyWorldGenRulesData>,
    pub replacement_biomes: Option<BiomeReplacementsData>,
    pub village_type: Option<u8>,
    pub surface_builder_data: Option<BiomeSurfaceBuilderData>,
    pub subsurface_builder_data: Option<BiomeSurfaceBuilderData>,
}

impl Encode for BiomeDefinitionChunkGenData {
    fn encode(&self, w: &mut Writer) {
        match &self.climate {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.consolidated_features {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.mountain_params {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.surface_material_adjustments {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.overworld_gen_rules {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.multinoise_gen_rules {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.legacy_world_gen_rules {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.replacement_biomes {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.village_type {
            Some(v) => {
                w.u8(1);
                w.u8(*v);
            }
            None => w.u8(0),
        }
        match &self.surface_builder_data {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.subsurface_builder_data {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeDefinitionData {
    pub id: u16,
    pub temperature: f32,
    pub downfall: f32,
    pub foliage_snow: f32,
    pub depth: f32,
    pub scale: f32,
    pub map_water_color_argb: i32,
    pub rain: bool,
    pub tags: Option<BiomeTagsData>,
    pub chunk_gen_data: Option<BiomeDefinitionChunkGenData>,
}

impl Encode for BiomeDefinitionData {
    fn encode(&self, w: &mut Writer) {
        w.u16(self.id);
        w.f32(self.temperature);
        w.f32(self.downfall);
        w.f32(self.foliage_snow);
        w.f32(self.depth);
        w.f32(self.scale);
        w.i32(self.map_water_color_argb);
        w.bool(self.rain);
        match &self.tags {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.chunk_gen_data {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeStringList {
    pub strings: Vec<String>,
}

impl Encode for BiomeStringList {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.strings.len() as u32);
        for v in &self.strings {
            w.string(v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeDefinitionList {
    pub map_of_biome_names_to_data: Vec<(u16, BiomeDefinitionData)>,
    pub string_list: BiomeStringList,
}

impl Encode for BiomeDefinitionList {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.map_of_biome_names_to_data.len() as u32);
        for (k, v) in &self.map_of_biome_names_to_data {
            w.u16(*k);
            v.encode(w);
        }
        self.string_list.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AvailableActorIdentifiers {
    pub identifier_list: Vec<u8>,
}

impl Encode for AvailableActorIdentifiers {
    fn encode(&self, w: &mut Writer) {
        w.raw(&self.identifier_list);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkItemInstanceDescriptorData {
    pub id: i32,
    pub stack_size: u16,
    pub aux_value: u32,
    pub block_runtime_id: i32,
    pub user_data_buffer: Vec<u8>,
}

impl Encode for NetworkItemInstanceDescriptorData {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.id);
        w.u16(self.stack_size);
        w.var_u32(self.aux_value);
        w.var_i32(self.block_runtime_id);
        w.bytes(&self.user_data_buffer);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreativeGroupInfoPayload {
    pub creative_category: u8,
    pub name: String,
    pub group_icon_item: NetworkItemInstanceDescriptorData,
}

impl Encode for CreativeGroupInfoPayload {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.creative_category);
        w.string(&self.name);
        self.group_icon_item.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreativeItemEntryPayload {
    pub creative_net_id: u32,
    pub item_instance: NetworkItemInstanceDescriptorData,
    pub group_index: u32,
}

impl Encode for CreativeItemEntryPayload {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.creative_net_id);
        self.item_instance.encode(w);
        w.var_u32(self.group_index);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreativeContent {
    pub groups: Vec<CreativeGroupInfoPayload>,
    pub entries: Vec<CreativeItemEntryPayload>,
}

impl Encode for CreativeContent {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.groups.len() as u32);
        for v in &self.groups {
            v.encode(w);
        }
        w.var_u32(self.entries.len() as u32);
        for v in &self.entries {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxelShapesSerializableCells {
    pub x_size: u8,
    pub y_size: u8,
    pub z_size: u8,
    pub storage: Vec<u8>,
}

impl Encode for VoxelShapesSerializableCells {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.x_size);
        w.u8(self.y_size);
        w.u8(self.z_size);
        w.var_u32(self.storage.len() as u32);
        for v in &self.storage {
            w.u8(*v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxelShapesSerializableVoxelShape {
    pub cells: VoxelShapesSerializableCells,
    pub x_coordinates: Vec<f32>,
    pub y_coordinates: Vec<f32>,
    pub z_coordinates: Vec<f32>,
}

impl Encode for VoxelShapesSerializableVoxelShape {
    fn encode(&self, w: &mut Writer) {
        self.cells.encode(w);
        w.var_u32(self.x_coordinates.len() as u32);
        for v in &self.x_coordinates {
            w.f32(*v);
        }
        w.var_u32(self.y_coordinates.len() as u32);
        for v in &self.y_coordinates {
            w.f32(*v);
        }
        w.var_u32(self.z_coordinates.len() as u32);
        for v in &self.z_coordinates {
            w.f32(*v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxelShapes {
    pub shapes: Vec<VoxelShapesSerializableVoxelShape>,
    pub name_map: Vec<(String, u16)>,
    pub custom_shape_count: u16,
}

impl Encode for VoxelShapes {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.shapes.len() as u32);
        for v in &self.shapes {
            v.encode(w);
        }
        w.var_u32(self.name_map.len() as u32);
        for (k, v) in &self.name_map {
            w.string(k);
            w.u16(*v);
        }
        w.u16(self.custom_shape_count);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JigsawStructureData {
    pub jigsaw_structure_data_tag: Vec<u8>,
}

impl Encode for JigsawStructureData {
    fn encode(&self, w: &mut Writer) {
        w.raw(&self.jigsaw_structure_data_tag);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FullContainerName {
    pub container_name: u8,
    pub dynamic_id: Option<u32>,
}

impl Encode for FullContainerName {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.container_name);
        match &self.dynamic_id {
            Some(v) => {
                w.u8(1);
                w.u32(*v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventoryContent {
    pub container_id: u32,
    pub slots: Vec<ItemStack>,
    pub full_container_name: FullContainerName,
    pub storage_item: ItemStack,
}

impl Encode for InventoryContent {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.container_id);
        w.var_u32(self.slots.len() as u32);
        for v in &self.slots {
            v.encode(w);
        }
        self.full_container_name.encode(w);
        self.storage_item.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventorySlot {
    pub container_id: u8,
    pub slot: u32,
    pub full_container_name: Option<FullContainerName>,
    pub storage_item: Option<ItemStack>,
    pub item: ItemStack,
}

impl Encode for InventorySlot {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.container_id);
        w.var_u32(self.slot);
        match &self.full_container_name {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        match &self.storage_item {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
        self.item.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MobEquipment {
    pub target_runtime_id: u64,
    pub item: ItemStack,
    pub slot: u8,
    pub selected_slot: u8,
    pub container_id: u8,
}

impl Encode for MobEquipment {
    fn encode(&self, w: &mut Writer) {
        w.var_u64(self.target_runtime_id);
        self.item.encode(w);
        w.u8(self.slot);
        w.u8(self.selected_slot);
        w.u8(self.container_id);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerHotbar {
    pub selected_slot: u32,
    pub container_id: u8,
    pub should_select_slot: bool,
}

impl Encode for PlayerHotbar {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.selected_slot);
        w.u8(self.container_id);
        w.bool(self.should_select_slot);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateBlock {
    pub block_position: BlockPos,
    pub block_runtime_id: u32,
    pub flags: u32,
    pub layer: u32,
}

impl Encode for UpdateBlock {
    fn encode(&self, w: &mut Writer) {
        self.block_position.encode(w);
        w.var_u32(self.block_runtime_id);
        w.var_u32(self.flags);
        w.var_u32(self.layer);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetPlayerGameType {
    pub player_game_type: i32,
}

impl Encode for SetPlayerGameType {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.player_game_type);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdatePlayerGameType {
    pub player_game_type: i32,
    pub target_player: i64,
    pub tick: u64,
}

impl Encode for UpdatePlayerGameType {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.player_game_type);
        w.var_i64(self.target_player);
        w.var_u64(self.tick);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedAbilitiesDataSerializedLayer {
    pub serialized_layer: u16,
    pub abilities_set: u32,
    pub ability_values: u32,
    pub fly_speed: f32,
    pub vertical_fly_speed: f32,
    pub walk_speed: f32,
}

impl Encode for SerializedAbilitiesDataSerializedLayer {
    fn encode(&self, w: &mut Writer) {
        w.u16(self.serialized_layer);
        w.u32(self.abilities_set);
        w.u32(self.ability_values);
        w.f32(self.fly_speed);
        w.f32(self.vertical_fly_speed);
        w.f32(self.walk_speed);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedAbilitiesData {
    pub target_player_raw_id: i64,
    pub player_permissions: i8,
    pub command_permissions: u8,
    pub layers: Vec<SerializedAbilitiesDataSerializedLayer>,
}

impl Encode for SerializedAbilitiesData {
    fn encode(&self, w: &mut Writer) {
        w.i64(self.target_player_raw_id);
        w.i8(self.player_permissions);
        w.u8(self.command_permissions);
        w.var_u32(self.layers.len() as u32);
        for v in &self.layers {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateAbilities {
    pub data: SerializedAbilitiesData,
}

impl Encode for UpdateAbilities {
    fn encode(&self, w: &mut Writer) {
        self.data.encode(w);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelEvent {
    pub event_id: i32,
    pub position: Vec3,
    pub data: i32,
}

impl Encode for LevelEvent {
    fn encode(&self, w: &mut Writer) {
        w.var_i32(self.event_id);
        self.position.encode(w);
        w.var_i32(self.data);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelSoundEvent {
    pub sound_event: String,
    pub position: Vec3,
    pub data: i32,
    pub actor_identifier: String,
    pub is_baby: bool,
    pub is_global: bool,
    pub actor_unique_id: i64,
    pub fire_at_position: Option<Vec3>,
}

impl Encode for LevelSoundEvent {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.sound_event);
        self.position.encode(w);
        w.var_i32(self.data);
        w.string(&self.actor_identifier);
        w.bool(self.is_baby);
        w.bool(self.is_global);
        w.i64(self.actor_unique_id);
        match &self.fire_at_position {
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BedrockSafetyRedactableString {
    pub unredacted: String,
    pub redacted: Option<String>,
}

impl Encode for BedrockSafetyRedactableString {
    fn encode(&self, w: &mut Writer) {
        w.string(&self.unredacted);
        match &self.redacted {
            Some(v) => {
                w.u8(1);
                w.string(v);
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStackResponseSlotInfo {
    pub requested_slot: u8,
    pub slot: u8,
    pub amount: u8,
    pub item_stack_net_id: Option<i32>,
    pub custom_name: BedrockSafetyRedactableString,
    pub durability_correction: i32,
}

impl Encode for ItemStackResponseSlotInfo {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.requested_slot);
        w.u8(self.slot);
        w.u8(self.amount);
        match &self.item_stack_net_id {
            Some(v) => {
                w.u8(1);
                w.var_i32(*v);
            }
            None => w.u8(0),
        }
        self.custom_name.encode(w);
        w.var_i32(self.durability_correction);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStackResponseContainerInfo {
    pub full_container_name: FullContainerName,
    pub slots: Vec<ItemStackResponseSlotInfo>,
}

impl Encode for ItemStackResponseContainerInfo {
    fn encode(&self, w: &mut Writer) {
        self.full_container_name.encode(w);
        w.var_u32(self.slots.len() as u32);
        for v in &self.slots {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStackResponseInfo {
    pub result: u8,
    pub client_request_id: i32,
    pub containers: Option<Vec<ItemStackResponseContainerInfo>>,
}

impl Encode for ItemStackResponseInfo {
    fn encode(&self, w: &mut Writer) {
        w.u8(self.result);
        w.var_i32(self.client_request_id);
        match &self.containers {
            Some(v) => {
                w.u8(1);
                w.var_u32(v.len() as u32);
                for v in v {
                    v.encode(w);
                }
            }
            None => w.u8(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStackResponse {
    pub responses: Vec<ItemStackResponseInfo>,
}

impl Encode for ItemStackResponse {
    fn encode(&self, w: &mut Writer) {
        w.var_u32(self.responses.len() as u32);
        for v in &self.responses {
            v.encode(w);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockActorData {
    pub block_position: BlockPos,
    pub actor_data_tags: Vec<u8>,
}

impl Encode for BlockActorData {
    fn encode(&self, w: &mut Writer) {
        self.block_position.encode(w);
        w.raw(&self.actor_data_tags);
    }
}

impl ServerPacket for StartGame {
    const ID: u32 = 11;
}

impl ServerPacket for ItemRegistry {
    const ID: u32 = 162;
}

impl ServerPacket for LevelChunk {
    const ID: u32 = 58;
}

impl ServerPacket for ChunkRadiusUpdated {
    const ID: u32 = 70;
}

impl ServerPacket for NetworkChunkPublisherUpdate {
    const ID: u32 = 121;
}

impl ServerPacket for BiomeDefinitionList {
    const ID: u32 = 122;
}

impl ServerPacket for AvailableActorIdentifiers {
    const ID: u32 = 119;
}

impl ServerPacket for CreativeContent {
    const ID: u32 = 145;
}

impl ServerPacket for VoxelShapes {
    const ID: u32 = 337;
}

impl ServerPacket for JigsawStructureData {
    const ID: u32 = 313;
}

impl ServerPacket for InventoryContent {
    const ID: u32 = 49;
}

impl ServerPacket for InventorySlot {
    const ID: u32 = 50;
}

impl ServerPacket for MobEquipment {
    const ID: u32 = 31;
}

impl ServerPacket for PlayerHotbar {
    const ID: u32 = 48;
}

impl ServerPacket for UpdateBlock {
    const ID: u32 = 21;
}

impl ServerPacket for SetPlayerGameType {
    const ID: u32 = 62;
}

impl ServerPacket for UpdatePlayerGameType {
    const ID: u32 = 151;
}

impl ServerPacket for UpdateAbilities {
    const ID: u32 = 187;
}

impl ServerPacket for LevelEvent {
    const ID: u32 = 25;
}

impl ServerPacket for LevelSoundEvent {
    const ID: u32 = 123;
}

impl ServerPacket for ItemStackResponse {
    const ID: u32 = 148;
}

impl ServerPacket for BlockActorData {
    const ID: u32 = 56;
}

/// Valori degli enum degli schemi (i campi sono interi: si usano queste costanti).
pub mod enums {
    pub mod chat_restriction_level {
        pub const NONE: u8 = 0;
        pub const DROPPED: u8 = 1;
        pub const DISABLED: u8 = 2;
    }
    pub mod command_permission_level {
        pub const ANY: u8 = 0;
        pub const GAME_DIRECTORS: u8 = 1;
        pub const ADMIN: u8 = 2;
        pub const HOST: u8 = 3;
        pub const OWNER: u8 = 4;
        pub const INTERNAL: u8 = 5;
    }
    pub mod container_enum_name {
        pub const ANVIL_INPUT_CONTAINER: u8 = 0;
        pub const ANVIL_MATERIAL_CONTAINER: u8 = 1;
        pub const ANVIL_RESULT_PREVIEW_CONTAINER: u8 = 2;
        pub const SMITHING_TABLE_INPUT_CONTAINER: u8 = 3;
        pub const SMITHING_TABLE_MATERIAL_CONTAINER: u8 = 4;
        pub const SMITHING_TABLE_RESULT_PREVIEW_CONTAINER: u8 = 5;
        pub const ARMOR_CONTAINER: u8 = 6;
        pub const LEVEL_ENTITY_CONTAINER: u8 = 7;
        pub const BEACON_PAYMENT_CONTAINER: u8 = 8;
        pub const BREWING_STAND_INPUT_CONTAINER: u8 = 9;
        pub const BREWING_STAND_RESULT_CONTAINER: u8 = 10;
        pub const BREWING_STAND_FUEL_CONTAINER: u8 = 11;
        pub const COMBINED_HOTBAR_AND_INVENTORY_CONTAINER: u8 = 12;
        pub const CRAFTING_INPUT_CONTAINER: u8 = 13;
        pub const CRAFTING_OUTPUT_PREVIEW_CONTAINER: u8 = 14;
        pub const RECIPE_CONSTRUCTION_CONTAINER: u8 = 15;
        pub const RECIPE_NATURE_CONTAINER: u8 = 16;
        pub const RECIPE_ITEMS_CONTAINER: u8 = 17;
        pub const RECIPE_FOOD_CONTAINER: u8 = 64;
        pub const RECIPE_BLOCKS_CONTAINER: u8 = 65;
        pub const RECIPE_FURNACE_ITEMS_CONTAINER: u8 = 66;
        pub const RECIPE_SEARCH_CONTAINER: u8 = 18;
        pub const RECIPE_SEARCH_BAR_CONTAINER: u8 = 19;
        pub const RECIPE_EQUIPMENT_CONTAINER: u8 = 20;
        pub const RECIPE_BOOK_CONTAINER: u8 = 21;
        pub const ENCHANTING_INPUT_CONTAINER: u8 = 22;
        pub const ENCHANTING_MATERIAL_CONTAINER: u8 = 23;
        pub const FURNACE_FUEL_CONTAINER: u8 = 24;
        pub const FURNACE_INGREDIENT_CONTAINER: u8 = 25;
        pub const FURNACE_RESULT_CONTAINER: u8 = 26;
        pub const HORSE_EQUIP_CONTAINER: u8 = 27;
        pub const HOTBAR_CONTAINER: u8 = 28;
        pub const INVENTORY_CONTAINER: u8 = 29;
        pub const SHULKER_BOX_CONTAINER: u8 = 30;
        pub const TRADE_INGREDIENT1_CONTAINER: u8 = 31;
        pub const TRADE_INGREDIENT2_CONTAINER: u8 = 32;
        pub const TRADE_RESULT_PREVIEW_CONTAINER: u8 = 33;
        pub const OFFHAND_CONTAINER: u8 = 34;
        pub const COMPOUND_CREATOR_INPUT: u8 = 35;
        pub const COMPOUND_CREATOR_OUTPUT_PREVIEW: u8 = 36;
        pub const ELEMENT_CONSTRUCTOR_OUTPUT_PREVIEW: u8 = 37;
        pub const MATERIAL_REDUCER_INPUT: u8 = 38;
        pub const MATERIAL_REDUCER_OUTPUT: u8 = 39;
        pub const LAB_TABLE_INPUT: u8 = 40;
        pub const LOOM_INPUT_CONTAINER: u8 = 41;
        pub const LOOM_DYE_CONTAINER: u8 = 42;
        pub const LOOM_MATERIAL_CONTAINER: u8 = 43;
        pub const LOOM_RESULT_PREVIEW_CONTAINER: u8 = 44;
        pub const BLAST_FURNACE_INGREDIENT_CONTAINER: u8 = 45;
        pub const SMOKER_INGREDIENT_CONTAINER: u8 = 46;
        pub const TRADE2_INGREDIENT1_CONTAINER: u8 = 47;
        pub const TRADE2_INGREDIENT2_CONTAINER: u8 = 48;
        pub const TRADE2_RESULT_PREVIEW_CONTAINER: u8 = 49;
        pub const GRINDSTONE_INPUT_CONTAINER: u8 = 50;
        pub const GRINDSTONE_ADDITIONAL_CONTAINER: u8 = 51;
        pub const GRINDSTONE_RESULT_PREVIEW_CONTAINER: u8 = 52;
        pub const STONECUTTER_INPUT_CONTAINER: u8 = 53;
        pub const STONECUTTER_RESULT_PREVIEW_CONTAINER: u8 = 54;
        pub const CARTOGRAPHY_INPUT_CONTAINER: u8 = 55;
        pub const CARTOGRAPHY_ADDITIONAL_CONTAINER: u8 = 56;
        pub const CARTOGRAPHY_RESULT_PREVIEW_CONTAINER: u8 = 57;
        pub const BARREL_CONTAINER: u8 = 58;
        pub const CURSOR_CONTAINER: u8 = 59;
        pub const CREATED_OUTPUT_CONTAINER: u8 = 60;
        pub const SMITHING_TABLE_TEMPLATE_CONTAINER: u8 = 61;
        pub const CRAFTER_LEVEL_ENTITY_CONTAINER: u8 = 62;
        pub const DYNAMIC_CONTAINER: u8 = 63;
        pub const RESERVED_CONTAINER_A: u8 = 67;
        pub const RESERVED_CONTAINER_B: u8 = 68;
        pub const RESERVED_CONTAINER_C: u8 = 69;
        pub const RESERVED_CONTAINER_D: u8 = 70;
    }
    pub mod coordinate_evaluation_order {
        pub const XYZ: i32 = 0;
        pub const XZY: i32 = 1;
        pub const YXZ: i32 = 2;
        pub const YZX: i32 = 3;
        pub const ZXY: i32 = 4;
        pub const ZYX: i32 = 5;
    }
    pub mod difficulty {
        pub const PEACEFUL: i32 = 0;
        pub const EASY: i32 = 1;
        pub const NORMAL: i32 = 2;
        pub const HARD: i32 = 3;
        pub const COUNT: i32 = 4;
        pub const UNKNOWN: i32 = 5;
    }
    pub mod editor_level_migration_version {
        pub const LEGACY: i8 = 0;
        pub const EDITOR_ACHIEVEMENTS: i8 = 1;
    }
    pub mod editor_world_type {
        pub const NON_EDITOR: i32 = 0;
        pub const EDITOR_PROJECT: i32 = 1;
        pub const EDITOR_TEST_LEVEL: i32 = 2;
        pub const EDITOR_REALMS_UPLOAD: i32 = 3;
    }
    pub mod education_edition_offer {
        pub const NONE: u32 = 0;
        pub const REST_OF_WORLD: u32 = 1;
        pub const CHINA_DEPRECATED: u32 = 2;
    }
    pub mod game_publish_setting {
        pub const NO_MULTI_PLAY: i32 = 0;
        pub const INVITE_ONLY: i32 = 1;
        pub const FRIENDS_ONLY: i32 = 2;
        pub const FRIENDS_OF_FRIENDS: i32 = 3;
        pub const PUBLIC: i32 = 4;
    }
    pub mod game_type {
        pub const UNDEFINED: i32 = -1;
        pub const SURVIVAL: i32 = 0;
        pub const CREATIVE: i32 = 1;
        pub const ADVENTURE: i32 = 2;
        pub const DEFAULT: i32 = 5;
        pub const SPECTATOR: i32 = 6;
        pub const WORLD_DEFAULT: i32 = 0;
    }
    pub mod generator_type {
        pub const LEGACY: i32 = 0;
        pub const OVERWORLD: i32 = 1;
        pub const FLAT: i32 = 2;
        pub const NETHER: i32 = 3;
        pub const THE_END: i32 = 4;
        pub const VOID: i32 = 5;
        pub const UNDEFINED: i32 = 6;
    }
    pub mod item_version {
        pub const LEGACY: i32 = 0;
        pub const DATA_DRIVEN: i32 = 1;
        pub const NONE: i32 = 2;
    }
    pub mod player_permission_level {
        pub const VISITOR: i8 = 0;
        pub const MEMBER: i8 = 1;
        pub const OPERATOR: i8 = 2;
        pub const CUSTOM: i8 = 3;
    }
    pub mod random_distribution_type {
        pub const SINGLE_VALUED: i32 = 0;
        pub const UNIFORM: i32 = 1;
        pub const GAUSSIAN: i32 = 2;
        pub const INVERSE_GAUSSIAN: i32 = 3;
        pub const FIXED_GRID: i32 = 4;
        pub const JITTERED_GRID: i32 = 5;
        pub const TRIANGLE: i32 = 6;
    }
    pub mod server_editor_connection_policy {
        pub const MATCH_WORLD_TYPE: i32 = 0;
        pub const EDITOR_ONLY: i32 = 1;
        pub const VANILLA_ONLY: i32 = 2;
        pub const MIXED: i32 = 3;
    }
    pub mod spawn_biome_type {
        pub const DEFAULT: i16 = 0;
        pub const USER_DEFINED: i16 = 1;
    }
    pub mod village_type {
        pub const DESERT: u8 = 0;
        pub const ICE: u8 = 1;
        pub const SAVANNA: u8 = 2;
        pub const TAIGA: u8 = 3;
        pub const DEFAULT: u8 = 4;
    }
}
