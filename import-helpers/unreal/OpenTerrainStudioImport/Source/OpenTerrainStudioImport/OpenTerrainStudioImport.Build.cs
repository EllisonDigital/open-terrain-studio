// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 EllisonDigital
using UnrealBuildTool;
public class OpenTerrainStudioImport : ModuleRules
{
    public OpenTerrainStudioImport(ReadOnlyTargetRules Target) : base(Target)
    {
        PCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;
        PublicDependencyModuleNames.AddRange(new[] { "Core", "CoreUObject", "Engine", "Landscape" });
        PrivateDependencyModuleNames.AddRange(new[] { "UnrealEd", "LandscapeEditor" });
    }
}
