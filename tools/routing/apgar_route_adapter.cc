#include <algorithm>
#include <array>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <limits>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

#include "apgar/board_ir/board.h"
#include "apgar/board_ir/stable_hash.h"
#include "apgar/candidates/route_candidate.h"
#include "apgar/geometry_compiler/compiled_board.h"
#include "apgar/routing/candidate_policy.h"
#include "apgar/routing/cpu_astar.h"
#include "apgar/routing/planar_route.h"
#include "nlohmann/json.hpp"

namespace {

using Json = nlohmann::ordered_json;
using apgar::board_ir::AxisAlignedBox64;
using apgar::board_ir::BoardData;
using apgar::board_ir::BoardSnapshot;
using apgar::board_ir::EntityRef;
using apgar::board_ir::Layer;
using apgar::board_ir::LayerId;
using apgar::board_ir::Net;
using apgar::board_ir::Obstacle;
using apgar::board_ir::Point64;
using apgar::board_ir::RoutingProfile;
using apgar::board_ir::StableHashBuilder;
using apgar::board_ir::Terminal;
using apgar::candidates::AdmitRouteCandidate;
using apgar::candidates::CandidateAdmissionContext;
using apgar::candidates::CandidateBackendKind;
using apgar::candidates::CandidateGeneratorKind;
using apgar::candidates::CandidateSchedulingIdentity;
using apgar::candidates::GeneratedRouteCandidate;
using apgar::candidates::RouteCandidate;
using apgar::geometry_compiler::ActiveRegion;
using apgar::geometry_compiler::CompiledBoard;
using apgar::geometry_compiler::CompilerProfile;
using apgar::geometry_compiler::DeterministicCosts;
using apgar::geometry_compiler::Direction;
using apgar::routing::CandidateGenerationPolicy;
using apgar::routing::CandidateObjective;
using apgar::routing::CpuRoute;
using apgar::routing::EdgeResourceKey;
using apgar::routing::NormalizedCandidateGenerationPolicy;
using apgar::routing::PlanarRouteRequest;
using apgar::routing::ResourcePenalty;

constexpr std::size_t kMaximumContractBytes = 64U * 1024U * 1024U;
constexpr std::string_view kRequestSchema = "circuitc.apgar_route_request";
constexpr std::string_view kResultSchema = "circuitc.apgar_route_result";
constexpr std::string_view kSourceRevision = "85a4f75b8c0c6142d319a8a743087f65ef9e9796";
constexpr std::string_view kContractIdentity =
    "apgar-board-ir-v1+geometry-compiler-v1+candidate-policy-v1+route-candidate-v1.0";
constexpr std::string_view kToolName = "circuitc-apgar-route";
constexpr std::string_view kToolVersion = "1";
constexpr std::string_view kDeviceClass = "cpu-reference-v1";
constexpr std::string_view kCompanionLayerIdentityDomain =
    "CIRCUITC-APGAR-COMPANION-LAYER-V1";

struct Invocation {
  std::string request_sha256;
  std::string executable_sha256;
};

[[nodiscard]] bool LowerHex(std::string_view value, std::size_t length) {
  return value.size() == length &&
         std::ranges::all_of(value, [](char character) {
           return (character >= '0' && character <= '9') ||
                  (character >= 'a' && character <= 'f');
         });
}

[[nodiscard]] Invocation ParseInvocation(int argc, char** argv) {
  if (argc != 5 || std::string_view(argv[1]) != "--request-sha256" ||
      std::string_view(argv[3]) != "--executable-sha256") {
    throw std::runtime_error(
        "usage: apgar_route_adapter --request-sha256 HEX --executable-sha256 HEX");
  }
  Invocation invocation{.request_sha256 = argv[2], .executable_sha256 = argv[4]};
  if (!LowerHex(invocation.request_sha256, 64) ||
      !LowerHex(invocation.executable_sha256, 64)) {
    throw std::runtime_error("request and executable identities must be SHA-256 hex");
  }
  return invocation;
}

[[nodiscard]] std::string ReadRequest() {
  std::string input;
  std::array<char, 8192> buffer{};
  while (std::cin.good()) {
    std::cin.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
    const std::streamsize count = std::cin.gcount();
    if (count <= 0) {
      break;
    }
    if (input.size() > kMaximumContractBytes - static_cast<std::size_t>(count)) {
      throw std::runtime_error("request exceeds the 64 MiB process-contract limit");
    }
    input.append(buffer.data(), static_cast<std::size_t>(count));
  }
  if (input.empty() || input.back() != '\n') {
    throw std::runtime_error("request must contain one canonical final LF");
  }
  return input;
}

void RequireKeys(const Json& value, std::initializer_list<std::string_view> keys,
                 std::string_view path) {
  if (!value.is_object() || value.size() != keys.size()) {
    throw std::runtime_error(std::string(path) + " has an invalid exact key set");
  }
  auto actual = value.begin();
  for (std::string_view expected : keys) {
    if (actual == value.end() || actual.key() != expected) {
      throw std::runtime_error(std::string(path) + " has a non-canonical key or key order");
    }
    ++actual;
  }
}

[[nodiscard]] std::string String(const Json& value, std::string_view path) {
  if (!value.is_string()) {
    throw std::runtime_error(std::string(path) + " must be a string");
  }
  return value.get<std::string>();
}

[[nodiscard]] std::uint64_t U64(const Json& value, std::string_view path) {
  if (value.is_number_unsigned()) {
    return value.get<std::uint64_t>();
  }
  if (value.is_number_integer()) {
    const std::int64_t integer = value.get<std::int64_t>();
    if (integer >= 0) {
      return static_cast<std::uint64_t>(integer);
    }
  }
  throw std::runtime_error(std::string(path) + " must be an unsigned integer");
}

[[nodiscard]] std::uint32_t U32(const Json& value, std::string_view path) {
  const std::uint64_t integer = U64(value, path);
  if (integer > std::numeric_limits<std::uint32_t>::max()) {
    throw std::runtime_error(std::string(path) + " exceeds uint32");
  }
  return static_cast<std::uint32_t>(integer);
}

[[nodiscard]] std::int64_t I64(const Json& value, std::string_view path) {
  if (value.is_number_integer()) {
    return value.get<std::int64_t>();
  }
  if (value.is_number_unsigned()) {
    const std::uint64_t integer = value.get<std::uint64_t>();
    if (integer <= static_cast<std::uint64_t>(std::numeric_limits<std::int64_t>::max())) {
      return static_cast<std::int64_t>(integer);
    }
  }
  throw std::runtime_error(std::string(path) + " must fit int64");
}

[[nodiscard]] EntityRef Ref(const Json& value, std::string_view path) {
  RequireKeys(value, {"id", "generation"}, path);
  return EntityRef{.id = U64(value.at("id"), path),
                   .generation = U32(value.at("generation"), path)};
}

[[nodiscard]] EntityRef CompanionLayerRef(const Layer& selected,
                                          LayerId routing_id) noexcept {
  StableHashBuilder hash;
  hash.AddString(kCompanionLayerIdentityDomain);
  hash.AddU64(selected.ref.id);
  hash.AddU32(selected.ref.generation);
  hash.AddU32(routing_id);
  return EntityRef{.id = hash.Finish(), .generation = 0};
}

[[nodiscard]] Point64 Point(const Json& value, std::string_view path) {
  RequireKeys(value, {"x", "y"}, path);
  return Point64{.x = I64(value.at("x"), path), .y = I64(value.at("y"), path)};
}

[[nodiscard]] AxisAlignedBox64 Box(const Json& value, std::string_view path) {
  RequireKeys(value, {"min", "max"}, path);
  return AxisAlignedBox64{.min = Point(value.at("min"), path),
                          .max = Point(value.at("max"), path)};
}

[[nodiscard]] std::uint8_t HeadingMask(const Json& value, std::string_view path) {
  if (!value.is_array() || value.size() != 3 || value[0] != "horizontal" ||
      value[1] != "vertical" || value[2] != "diagonal45") {
    throw std::runtime_error(std::string(path) + " must be exact H/V/45 headings");
  }
  return apgar::board_ir::kM1HeadingMask;
}

[[nodiscard]] Direction ParseDirection(const Json& value, std::string_view path) {
  const std::string direction = String(value, path);
  if (direction == "east") return Direction::kEast;
  if (direction == "north_east") return Direction::kNorthEast;
  if (direction == "north") return Direction::kNorth;
  if (direction == "north_west") return Direction::kNorthWest;
  if (direction == "west") return Direction::kWest;
  if (direction == "south_west") return Direction::kSouthWest;
  if (direction == "south") return Direction::kSouth;
  if (direction == "south_east") return Direction::kSouthEast;
  throw std::runtime_error(std::string(path) + " contains an unknown direction");
}

[[nodiscard]] EdgeResourceKey Resource(const Json& value, std::string_view path) {
  RequireKeys(value, {"layer", "lattice_x", "lattice_y", "direction"}, path);
  return EdgeResourceKey{.layer = U32(value.at("layer"), path),
                         .lattice_x = I64(value.at("lattice_x"), path),
                         .lattice_y = I64(value.at("lattice_y"), path),
                         .direction = ParseDirection(value.at("direction"), path)};
}

[[nodiscard]] CandidateObjective Objective(const Json& value, std::string_view path) {
  const std::string objective = String(value, path);
  if (objective == "base_scalar_cost") return CandidateObjective::kBaseScalarCost;
  if (objective == "length_biased") return CandidateObjective::kLengthBiased;
  if (objective == "bend_biased") return CandidateObjective::kBendBiased;
  if (objective == "resource_diverse") return CandidateObjective::kResourceDiverse;
  throw std::runtime_error(std::string(path) + " contains an unknown objective");
}

[[nodiscard]] CandidateGenerationPolicy Policy(const Json& value, std::string_view path) {
  RequireKeys(value,
              {"schema_version", "objective", "deterministic_seed", "candidate_ordinal",
               "orthogonal_step_surcharge", "diagonal_step_surcharge", "bend_surcharge",
               "banned_resources", "resource_penalties"},
              path);
  CandidateGenerationPolicy policy{
      .schema_version = U32(value.at("schema_version"), path),
      .objective = Objective(value.at("objective"), path),
      .deterministic_seed = U64(value.at("deterministic_seed"), path),
      .candidate_ordinal = U32(value.at("candidate_ordinal"), path),
      .orthogonal_step_surcharge = U64(value.at("orthogonal_step_surcharge"), path),
      .diagonal_step_surcharge = U64(value.at("diagonal_step_surcharge"), path),
      .bend_surcharge = U64(value.at("bend_surcharge"), path),
      .banned_resources = {},
      .resource_penalties = {},
  };
  for (const Json& resource : value.at("banned_resources")) {
    policy.banned_resources.push_back(Resource(resource, path));
  }
  for (const Json& item : value.at("resource_penalties")) {
    RequireKeys(item, {"resource", "additional_cost"}, path);
    policy.resource_penalties.push_back(ResourcePenalty{
        .resource = Resource(item.at("resource"), path),
        .additional_cost = U64(item.at("additional_cost"), path),
    });
  }
  return policy;
}

[[nodiscard]] BoardData Board(const Json& request) {
  BoardData board{
      .schema_version = 1,
      .dbu_per_millimeter = I64(request.at("dbu_per_millimeter"), "dbu_per_millimeter"),
      .revision = U64(request.at("board_revision"), "board_revision"),
      .adapter_name = String(request.at("adapter_name"), "adapter_name"),
      .adapter_version = String(request.at("adapter_version"), "adapter_version"),
      .layers = {},
      .nets = {},
      .terminals = {},
      .obstacles = {},
      .routing_profile = {},
  };
  for (const Json& item : request.at("layers")) {
    RequireKeys(item, {"reference", "routing_id", "name", "physical_order", "side", "routable"},
                "layers[]");
    if (!item.at("routable").is_boolean() || !item.at("routable").get<bool>()) {
      throw std::runtime_error("layers[].routable must be true");
    }
    board.layers.push_back(Layer{
        .ref = Ref(item.at("reference"), "layers[].reference"),
        .routing_id = U32(item.at("routing_id"), "layers[].routing_id"),
        .name = String(item.at("name"), "layers[].name"),
        .physical_order = static_cast<std::int32_t>(I64(item.at("physical_order"),
                                                       "layers[].physical_order")),
        .type = apgar::board_ir::LayerType::kSignal,
        .routable = true,
    });
  }
  if (board.layers.size() != 1U ||
      (board.layers.front().routing_id != 0U && board.layers.front().routing_id != 31U)) {
    throw std::runtime_error("request v1 requires exactly one front or back routing layer");
  }
  const Layer selected = board.layers.front();
  const LayerId companion_routing_id = selected.routing_id == 0U ? 31U : 0U;
  board.layers.push_back(Layer{
      .ref = CompanionLayerRef(selected, companion_routing_id),
      .routing_id = companion_routing_id,
      .name = companion_routing_id == 0U ? "F.Cu" : "B.Cu",
      .physical_order = companion_routing_id == 0U ? 0 : 1,
      .type = apgar::board_ir::LayerType::kSignal,
      .routable = true,
  });
  for (const Json& item : request.at("nets")) {
    RequireKeys(item, {"reference", "name", "terminals"}, "nets[]");
    Net net{.ref = Ref(item.at("reference"), "nets[].reference"),
            .name = String(item.at("name"), "nets[].name"),
            .terminals = {}};
    for (const Json& terminal : item.at("terminals")) {
      net.terminals.push_back(Ref(terminal, "nets[].terminals[]"));
    }
    board.nets.push_back(std::move(net));
  }
  for (const Json& item : request.at("terminals")) {
    RequireKeys(item,
                {"reference", "net", "component_path", "pad", "center", "connection_region",
                 "layers"},
                "terminals[]");
    Terminal terminal{
        .ref = Ref(item.at("reference"), "terminals[].reference"),
        .net = Ref(item.at("net"), "terminals[].net"),
        .component = String(item.at("component_path"), "terminals[].component_path"),
        .pin = String(item.at("pad"), "terminals[].pad"),
        .center = Point(item.at("center"), "terminals[].center"),
        .connection_region = Box(item.at("connection_region"), "terminals[].connection_region"),
        .layers = {},
    };
    for (const Json& layer : item.at("layers")) {
      terminal.layers.push_back(U32(layer, "terminals[].layers[]"));
    }
    board.terminals.push_back(std::move(terminal));
  }
  for (const Json& item : request.at("obstacles")) {
    RequireKeys(item, {"reference", "layer", "bounds", "owner_net", "provenance"},
                "obstacles[]");
    std::optional<EntityRef> owner;
    if (!item.at("owner_net").is_null()) {
      owner = Ref(item.at("owner_net"), "obstacles[].owner_net");
    }
    board.obstacles.push_back(Obstacle{
        .ref = Ref(item.at("reference"), "obstacles[].reference"),
        .layer = U32(item.at("layer"), "obstacles[].layer"),
        .bounds = Box(item.at("bounds"), "obstacles[].bounds"),
        .owner_net = owner,
        .provenance = String(item.at("provenance"), "obstacles[].provenance"),
    });
  }
  const Json& profile = request.at("routing_profile");
  RequireKeys(profile,
              {"net", "nominal_width_dbu", "clearance_dbu", "allowed_layers",
               "allowed_headings"},
              "routing_profile");
  board.routing_profile = RoutingProfile{
      .net = Ref(profile.at("net"), "routing_profile.net"),
      .nominal_width = I64(profile.at("nominal_width_dbu"), "routing_profile.nominal_width_dbu"),
      .clearance = I64(profile.at("clearance_dbu"), "routing_profile.clearance_dbu"),
      .allowed_layers = {},
      .allowed_headings = HeadingMask(profile.at("allowed_headings"),
                                      "routing_profile.allowed_headings"),
  };
  for (const Json& layer : profile.at("allowed_layers")) {
    board.routing_profile.allowed_layers.push_back(U32(layer, "routing_profile.allowed_layers[]"));
  }
  return board;
}

[[nodiscard]] CompilerProfile Compiler(const Json& request) {
  const Json& value = request.at("compiler_profile");
  RequireKeys(value,
              {"schema_version", "lattice_origin", "lattice_step_dbu", "tile_width_nodes",
               "tile_height_nodes", "compilation_roi", "active_regions", "allowed_headings",
               "costs"},
              "compiler_profile");
  CompilerProfile profile{
      .schema_version = U32(value.at("schema_version"), "compiler_profile.schema_version"),
      .lattice_origin = Point(value.at("lattice_origin"), "compiler_profile.lattice_origin"),
      .lattice_step = I64(value.at("lattice_step_dbu"), "compiler_profile.lattice_step_dbu"),
      .tile_width_nodes = U32(value.at("tile_width_nodes"), "compiler_profile.tile_width_nodes"),
      .tile_height_nodes = U32(value.at("tile_height_nodes"), "compiler_profile.tile_height_nodes"),
      .compilation_roi = Box(value.at("compilation_roi"), "compiler_profile.compilation_roi"),
      .active_regions = {},
      .heading_mask = HeadingMask(value.at("allowed_headings"),
                                  "compiler_profile.allowed_headings"),
      .costs = {},
  };
  for (const Json& item : value.at("active_regions")) {
    RequireKeys(item, {"layer", "bounds"}, "compiler_profile.active_regions[]");
    profile.active_regions.push_back(ActiveRegion{
        .layer = U32(item.at("layer"), "compiler_profile.active_regions[].layer"),
        .bounds = Box(item.at("bounds"), "compiler_profile.active_regions[].bounds"),
    });
  }
  const Json& costs = value.at("costs");
  RequireKeys(costs, {"orthogonal_step", "diagonal_step", "bend"}, "compiler_profile.costs");
  profile.costs = DeterministicCosts{
      .orthogonal_step = U32(costs.at("orthogonal_step"), "compiler_profile.costs.orthogonal_step"),
      .diagonal_step = U32(costs.at("diagonal_step"), "compiler_profile.costs.diagonal_step"),
      .bend = U32(costs.at("bend"), "compiler_profile.costs.bend"),
  };
  return profile;
}

[[nodiscard]] PlanarRouteRequest RouteRequest(const Json& request) {
  const Json& value = request.at("planar_route");
  RequireKeys(value,
              {"net", "start", "goal", "start_layer", "goal_layer", "candidate_policy",
               "scheduling"},
              "planar_route");
  return PlanarRouteRequest{
      .net = Ref(value.at("net"), "planar_route.net"),
      .start = Point(value.at("start"), "planar_route.start"),
      .goal = Point(value.at("goal"), "planar_route.goal"),
      .start_layer = U32(value.at("start_layer"), "planar_route.start_layer"),
      .goal_layer = U32(value.at("goal_layer"), "planar_route.goal_layer"),
      .candidate_policy = Policy(value.at("candidate_policy"), "planar_route.candidate_policy"),
  };
}

[[nodiscard]] CandidateSchedulingIdentity Scheduling(const Json& request) {
  const Json& value = request.at("planar_route").at("scheduling");
  RequireKeys(value, {"batch_identity", "query_identity"}, "planar_route.scheduling");
  return CandidateSchedulingIdentity{
      .batch_identity = U64(value.at("batch_identity"), "planar_route.scheduling.batch_identity"),
      .query_identity = U64(value.at("query_identity"), "planar_route.scheduling.query_identity"),
  };
}

void ValidateRequest(const Json& request, const std::string& input) {
  RequireKeys(request,
              {"schema_name", "schema_version", "design_name", "design_fingerprint_sha256",
               "request_path", "request_identity_sha256", "expected_apgar_source_revision",
               "expected_apgar_contract_identity", "dbu_per_millimeter", "board_revision",
               "adapter_name", "adapter_version", "layers", "nets", "terminals", "obstacles",
               "routing_profile", "compiler_profile", "planar_route", "resource_limits",
               "unsupported_host_rules"},
              "request");
  if (request.at("schema_name") != kRequestSchema || request.at("schema_version") != 1 ||
      request.at("expected_apgar_source_revision") != kSourceRevision ||
      request.at("expected_apgar_contract_identity") != kContractIdentity) {
    throw std::runtime_error("request schema or pinned APGAR identity is unsupported");
  }
  const Json& limits = request.at("resource_limits");
  RequireKeys(limits,
              {"timeout_milliseconds", "stdout_bytes", "stderr_bytes", "diagnostic_bytes",
               "candidate_primitives", "expanded_resource_edges"},
              "resource_limits");
  if (U64(limits.at("stdout_bytes"), "resource_limits.stdout_bytes") > kMaximumContractBytes) {
    throw std::runtime_error("request stdout bound exceeds process-contract ceiling");
  }
  const std::string rendered = request.dump() + "\n";
  if (rendered != input) {
    throw std::runtime_error("request bytes are not canonical compact ordered JSON with final LF");
  }
}

[[nodiscard]] Json Tool(const Invocation& invocation) {
  Json value = Json::object();
  value["name"] = kToolName;
  value["version"] = kToolVersion;
  value["contract_identity"] = kContractIdentity;
  value["source_revision"] = kSourceRevision;
  value["executable_sha256"] = invocation.executable_sha256;
  value["device_class"] = kDeviceClass;
  return value;
}

[[nodiscard]] Json Replay(const Json& request) {
  Json value = Json::object();
  value["design_fingerprint_sha256"] = request.at("design_fingerprint_sha256");
  value["request_identity_sha256"] = request.at("request_identity_sha256");
  value["board_revision"] = request.at("board_revision");
  value["deterministic_seed"] =
      request.at("planar_route").at("candidate_policy").at("deterministic_seed");
  value["batch_identity"] = request.at("planar_route").at("scheduling").at("batch_identity");
  value["query_identity"] = request.at("planar_route").at("scheduling").at("query_identity");
  return value;
}

[[nodiscard]] Json ResultRoot(const Json& request, const Invocation& invocation) {
  Json result = Json::object();
  result["schema_name"] = kResultSchema;
  result["schema_version"] = 1;
  result["request_sha256"] = invocation.request_sha256;
  result["request_path"] = request.at("request_path");
  result["tool"] = Tool(invocation);
  result["replay"] = Replay(request);
  return result;
}

[[nodiscard]] Json Failure(const Json& request, const Invocation& invocation,
                           std::string_view status, std::string_view code,
                           std::string_view message) {
  Json result = ResultRoot(request, invocation);
  Json diagnostic = Json::object();
  diagnostic["code"] = code;
  diagnostic["path"] = request.at("request_path");
  diagnostic["message"] = message;
  Json outcome = Json::object();
  outcome["kind"] = "failure";
  outcome["status"] = status;
  outcome["diagnostic"] = std::move(diagnostic);
  result["outcome"] = std::move(outcome);
  return result;
}

[[nodiscard]] std::string Hex64(std::uint64_t value) {
  std::ostringstream output;
  output << std::hex << std::setfill('0') << std::setw(16) << value;
  return output.str();
}

[[nodiscard]] std::string Hex128(apgar::candidates::Hash128 value) {
  return Hex64(value.high) + Hex64(value.low);
}

[[nodiscard]] Json RefJson(EntityRef value) {
  Json result = Json::object();
  result["id"] = value.id;
  result["generation"] = value.generation;
  return result;
}

[[nodiscard]] Json PointJson(Point64 value) {
  Json result = Json::object();
  result["x"] = value.x;
  result["y"] = value.y;
  return result;
}

[[nodiscard]] std::string DirectionName(Direction direction) {
  switch (direction) {
    case Direction::kEast: return "east";
    case Direction::kNorthEast: return "north_east";
    case Direction::kNorth: return "north";
    case Direction::kNorthWest: return "north_west";
    case Direction::kWest: return "west";
    case Direction::kSouthWest: return "south_west";
    case Direction::kSouth: return "south";
    case Direction::kSouthEast: return "south_east";
  }
  throw std::runtime_error("unknown APGAR direction");
}

[[nodiscard]] Json ResourceJson(EdgeResourceKey value) {
  Json result = Json::object();
  result["layer"] = value.layer;
  result["lattice_x"] = value.lattice_x;
  result["lattice_y"] = value.lattice_y;
  result["direction"] = DirectionName(value.direction);
  return result;
}

[[nodiscard]] std::string ObjectiveName(CandidateObjective objective) {
  switch (objective) {
    case CandidateObjective::kBaseScalarCost: return "base_scalar_cost";
    case CandidateObjective::kLengthBiased: return "length_biased";
    case CandidateObjective::kBendBiased: return "bend_biased";
    case CandidateObjective::kResourceDiverse: return "resource_diverse";
  }
  throw std::runtime_error("unknown APGAR candidate objective");
}

[[nodiscard]] Json PolicyJson(const CandidateGenerationPolicy& policy) {
  Json result = Json::object();
  result["schema_version"] = policy.schema_version;
  result["objective"] = ObjectiveName(policy.objective);
  result["deterministic_seed"] = policy.deterministic_seed;
  result["candidate_ordinal"] = policy.candidate_ordinal;
  result["orthogonal_step_surcharge"] = policy.orthogonal_step_surcharge;
  result["diagonal_step_surcharge"] = policy.diagonal_step_surcharge;
  result["bend_surcharge"] = policy.bend_surcharge;
  result["banned_resources"] = Json::array();
  for (const EdgeResourceKey& resource : policy.banned_resources) {
    result["banned_resources"].push_back(ResourceJson(resource));
  }
  result["resource_penalties"] = Json::array();
  for (const ResourcePenalty& penalty : policy.resource_penalties) {
    Json item = Json::object();
    item["resource"] = ResourceJson(penalty.resource);
    item["additional_cost"] = penalty.additional_cost;
    result["resource_penalties"].push_back(std::move(item));
  }
  return result;
}

[[nodiscard]] Json CandidateJson(const GeneratedRouteCandidate& candidate,
                                 std::int64_t width_dbu) {
  Json result = Json::object();
  result["schema_major"] = candidate.schema_major;
  result["schema_minor"] = candidate.schema_minor;
  result["id"] = Hex128(candidate.id);
  result["net"] = RefJson(candidate.net);
  result["intended_terminals"] =
      Json::array({RefJson(candidate.intended_terminals[0]), RefJson(candidate.intended_terminals[1])});
  Json associations = Json::object();
  associations["board_content_hash"] = candidate.associations.board_content_hash;
  associations["compiler_profile_fingerprint"] =
      candidate.associations.compiler_profile_fingerprint;
  associations["geometry_compiler_version"] = candidate.associations.geometry_compiler_version;
  associations["routing_profile_fingerprint"] =
      candidate.associations.routing_profile_fingerprint;
  associations["rule_bucket_identity"] = candidate.associations.rule_bucket_identity;
  result["associations"] = std::move(associations);
  result["geometry_schema_version"] = candidate.geometry_schema_version;
  result["resource_schema_version"] = candidate.resource_schema_version;
  result["policy"] = PolicyJson(candidate.policy);
  result["policy_identity"] = candidate.policy_identity;
  Json provenance = Json::object();
  switch (candidate.provenance.generator) {
    case CandidateGeneratorKind::kCpuAStar: provenance["generator"] = "cpu_a_star"; break;
    case CandidateGeneratorKind::kCudaFrontier: provenance["generator"] = "cuda_frontier"; break;
    case CandidateGeneratorKind::kCudaSweep: provenance["generator"] = "cuda_sweep"; break;
  }
  provenance["generator_version"] = candidate.provenance.generator_version;
  provenance["backend"] = candidate.provenance.backend == CandidateBackendKind::kCpu ? "cpu" : "cuda";
  provenance["supported_device_class"] = candidate.provenance.supported_device_class;
  provenance["deterministic_seed"] = candidate.provenance.deterministic_seed;
  provenance["batch_identity"] = candidate.provenance.batch_identity;
  provenance["query_identity"] = candidate.provenance.query_identity;
  provenance["candidate_ordinal"] = candidate.provenance.candidate_ordinal;
  result["provenance"] = std::move(provenance);
  result["geometry"] = Json::array();
  for (const apgar::candidates::CandidatePrimitive& primitive : candidate.geometry) {
    const auto* line = std::get_if<apgar::candidates::ExactLinePrimitive>(&primitive);
    if (line == nullptr) {
      throw std::runtime_error("APGAR candidate contains unsupported non-line geometry");
    }
    Json value = Json::object();
    value["layer"] = line->layer;
    value["start"] = PointJson(line->centerline.start);
    value["end"] = PointJson(line->centerline.end);
    value["width_dbu"] = width_dbu;
    result["geometry"].push_back(std::move(value));
  }
  result["resources"] = Json::array();
  for (const apgar::candidates::PhysicalEdgeSpan& resource : candidate.resources) {
    Json value = Json::object();
    value["layer"] = resource.layer;
    value["lattice_x"] = resource.lattice_x;
    value["lattice_y"] = resource.lattice_y;
    value["direction"] = DirectionName(resource.direction);
    value["edge_count"] = resource.edge_count;
    value["usage_units"] = resource.usage_units;
    result["resources"].push_back(std::move(value));
  }
  Json metrics = Json::object();
  metrics["scalar_policy_cost"] = candidate.metrics.scalar_policy_cost;
  metrics["intrinsic_base_cost"] = candidate.metrics.intrinsic_base_cost;
  metrics["orthogonal_step_count"] = candidate.metrics.orthogonal_step_count;
  metrics["diagonal_step_count"] = candidate.metrics.diagonal_step_count;
  metrics["bend_count"] = candidate.metrics.bend_count;
  metrics["line_primitive_count"] = candidate.metrics.line_primitive_count;
  metrics["via_count"] = candidate.metrics.via_count;
  metrics["axis_aligned_length_dbu"] = candidate.metrics.axis_aligned_length_dbu;
  metrics["diagonal_projection_dbu"] = candidate.metrics.diagonal_projection_dbu;
  result["metrics"] = std::move(metrics);
  Json constraints = Json::object();
  constraints["supported_hard_constraints_satisfied"] =
      candidate.constraints.supported_hard_constraints_satisfied;
  constraints["unsupported_rules_remain"] = candidate.constraints.unsupported_rules_remain;
  constraints["connected_intended_terminal_count"] =
      candidate.constraints.connected_intended_terminal_count;
  switch (candidate.constraints.exact_validation_code) {
    case apgar::candidates::CandidateExactValidationCode::kPassed:
      constraints["exact_validation_status"] = "passed";
      break;
    case apgar::candidates::CandidateExactValidationCode::kUnsupportedGeometry:
      constraints["exact_validation_status"] = "unsupported_geometry";
      break;
    case apgar::candidates::CandidateExactValidationCode::kInvalidGeometry:
      constraints["exact_validation_status"] = "invalid_geometry";
      break;
    case apgar::candidates::CandidateExactValidationCode::kExactRuleViolation:
      constraints["exact_validation_status"] = "exact_rule_violation";
      break;
  }
  result["constraints"] = std::move(constraints);
  result["geometry_signature"] = Hex128(candidate.geometry_signature);
  result["resource_signature"] = Hex128(candidate.resource_signature);
  result["payload_checksum"] = Hex64(candidate.payload_checksum);
  result["logical_bytes"] = candidate.logical_bytes;
  return result;
}

[[nodiscard]] Json Completed(const Json& request, const Invocation& invocation,
                             const RouteCandidate& admitted) {
  const GeneratedRouteCandidate& candidate = admitted.data();
  Json result = ResultRoot(request, invocation);
  Json outcome = Json::object();
  outcome["kind"] = "completed";
  outcome["selected_candidate_id"] = Hex128(candidate.id);
  outcome["candidates"] = Json::array({CandidateJson(
      candidate, I64(request.at("routing_profile").at("nominal_width_dbu"),
                     "routing_profile.nominal_width_dbu"))});
  result["outcome"] = std::move(outcome);
  return result;
}

void Emit(const Json& result) {
  const std::string output = result.dump() + "\n";
  if (output.size() > kMaximumContractBytes) {
    throw std::runtime_error("result exceeds the 64 MiB process-contract limit");
  }
  std::cout << output;
}

int Run(const Json& request, const Invocation& invocation) {
  auto board_result = apgar::board_ir::CreateBoardSnapshot(Board(request));
  if (std::holds_alternative<apgar::board_ir::BoardValidationError>(board_result)) {
    Emit(Failure(request, invocation, "board_validation_failed", "CC-APGAR-BOARD-001",
                 "APGAR rejected the exact Board IR request"));
    return 0;
  }
  BoardSnapshot board = std::get<BoardSnapshot>(std::move(board_result));
  auto compiled_result = apgar::geometry_compiler::CompileBoard(board, Compiler(request));
  if (std::holds_alternative<apgar::geometry_compiler::CompileError>(compiled_result)) {
    Emit(Failure(request, invocation, "compilation_failed", "CC-APGAR-COMPILE-001",
                 "APGAR could not compile the exact routing lattice"));
    return 0;
  }
  CompiledBoard compiled = std::get<CompiledBoard>(std::move(compiled_result));
  PlanarRouteRequest route_request = RouteRequest(request);
  auto policy_result =
      apgar::routing::NormalizeCandidateGenerationPolicy(compiled, route_request.candidate_policy);
  if (std::holds_alternative<apgar::routing::CandidatePolicyError>(policy_result)) {
    Emit(Failure(request, invocation, "policy_rejected", "CC-APGAR-POLICY-001",
                 "APGAR rejected the exact candidate policy"));
    return 0;
  }
  NormalizedCandidateGenerationPolicy policy =
      std::get<NormalizedCandidateGenerationPolicy>(std::move(policy_result));
  auto route_result = apgar::routing::RouteWithCpuAStar(board, compiled, route_request);
  if (std::holds_alternative<apgar::routing::RouteFailure>(route_result)) {
    Emit(Failure(request, invocation, "route_not_found", "CC-APGAR-ROUTE-001",
                 "APGAR CPU A* did not produce an exact admitted route"));
    return 0;
  }
  CpuRoute route = std::get<CpuRoute>(std::move(route_result));
  auto draft_result = apgar::candidates::BuildGeneratedCandidateFromCpuRoute(
      board, compiled, route_request, policy, route, Scheduling(request));
  if (std::holds_alternative<apgar::candidates::CandidateRejection>(draft_result)) {
    Emit(Failure(request, invocation, "candidate_rejected", "CC-APGAR-CANDIDATE-001",
                 "APGAR rejected CPU route candidate construction"));
    return 0;
  }
  GeneratedRouteCandidate draft =
      std::get<GeneratedRouteCandidate>(std::move(draft_result));
  CandidateAdmissionContext context{.board = board,
                                    .compiled_board = compiled,
                                    .request = route_request};
  auto admission = AdmitRouteCandidate(context, std::move(draft));
  if (std::holds_alternative<apgar::candidates::CandidateRejection>(admission)) {
    Emit(Failure(request, invocation, "candidate_rejected", "CC-APGAR-ADMISSION-001",
                 "APGAR exact candidate admission failed"));
    return 0;
  }
  RouteCandidate candidate = std::get<RouteCandidate>(std::move(admission));
  const std::uint64_t primitive_limit =
      U64(request.at("resource_limits").at("candidate_primitives"),
          "resource_limits.candidate_primitives");
  const std::uint64_t edge_limit =
      U64(request.at("resource_limits").at("expanded_resource_edges"),
          "resource_limits.expanded_resource_edges");
  std::uint64_t expanded_edges = 0;
  for (const apgar::candidates::PhysicalEdgeSpan& span : candidate.data().resources) {
    if (span.edge_count > edge_limit - expanded_edges) {
      Emit(Failure(request, invocation, "resource_limit_exceeded", "CC-APGAR-RESOURCE-001",
                   "APGAR candidate exceeds the request resource bound"));
      return 0;
    }
    expanded_edges += span.edge_count;
  }
  if (candidate.data().geometry.size() > primitive_limit) {
    Emit(Failure(request, invocation, "resource_limit_exceeded", "CC-APGAR-RESOURCE-001",
                 "APGAR candidate exceeds the request primitive bound"));
    return 0;
  }
  Emit(Completed(request, invocation, candidate));
  return 0;
}

}  // namespace

int main(int argc, char** argv) {
  try {
    const Invocation invocation = ParseInvocation(argc, argv);
    const std::string input = ReadRequest();
    const Json request = Json::parse(input);
    ValidateRequest(request, input);
    return Run(request, invocation);
  } catch (const std::exception& error) {
    std::cerr << "CC-APGAR-PROCESS-001: " << error.what() << '\n';
    return 2;
  }
}
